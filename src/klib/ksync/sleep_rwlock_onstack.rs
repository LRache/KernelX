use core::cell::UnsafeCell;
use core::ops::{Deref, DerefMut};
use core::pin::pin;
use core::sync::atomic::{AtomicUsize, Ordering, fence};

use spin::mutex::SpinMutex;

use crate::kernel::event::Event;
use crate::kernel::scheduler::{self, current};

use super::onstack_waiter::{WaitList, WaiterOnStack};

const WRITER_LOCKED: usize = 1 << (usize::BITS as usize - 1);
const CONTENDED: usize = 1 << (usize::BITS as usize - 2);
const READER_MASK: usize = CONTENDED - 1;

#[derive(Clone, Copy, PartialEq, Eq)]
enum WaitKind {
    Read,
    Write,
}

pub struct SleepRwLockOnStackReadGuard<'a, T> {
    lock: &'a SleepRwLockOnStack<T>,
}

impl<T> Deref for SleepRwLockOnStackReadGuard<'_, T> {
    type Target = T;

    fn deref(&self) -> &Self::Target {
        // SAFETY: the read guard holds one shared lock ownership, and writers
        // cannot acquire the lock until all read guards have been dropped.
        unsafe { &*self.lock.data.get() }
    }
}

impl<T> Drop for SleepRwLockOnStackReadGuard<'_, T> {
    fn drop(&mut self) {
        self.lock.read_unlock();
    }
}

pub struct SleepRwLockOnStackWriteGuard<'a, T> {
    lock: &'a SleepRwLockOnStack<T>,
}

impl<T> Deref for SleepRwLockOnStackWriteGuard<'_, T> {
    type Target = T;

    fn deref(&self) -> &Self::Target {
        // SAFETY: the write guard has exclusive lock ownership.
        unsafe { &*self.lock.data.get() }
    }
}

impl<T> DerefMut for SleepRwLockOnStackWriteGuard<'_, T> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        // SAFETY: the write guard has exclusive lock ownership.
        unsafe { &mut *self.lock.data.get() }
    }
}

impl<T> Drop for SleepRwLockOnStackWriteGuard<'_, T> {
    fn drop(&mut self) {
        self.lock.write_unlock();
    }
}

pub struct SleepRwLockOnStack<T> {
    state: AtomicUsize,
    waiters: SpinMutex<WaitList<WaitKind>>,
    data: UnsafeCell<T>,
    name: &'static str,
}

impl<T> SleepRwLockOnStack<T> {
    pub const fn new(data: T, name: &'static str) -> Self {
        Self {
            state: AtomicUsize::new(0),
            waiters: SpinMutex::new(WaitList::new()),
            data: UnsafeCell::new(data),
            name,
        }
    }

    pub fn read(&self) -> SleepRwLockOnStackReadGuard<'_, T> {
        if self.try_read() {
            return SleepRwLockOnStackReadGuard { lock: self };
        }

        let mut waiter = pin!(WaiterOnStack::new(current::task().clone(), WaitKind::Read));
        {
            let mut waiters = self.waiters.lock();
            loop {
                if self.try_read() {
                    return SleepRwLockOnStackReadGuard { lock: self };
                }
                if self.mark_contended(WaitKind::Read) {
                    break;
                }
            }
            // PERF_DEBUG(scheduler-time): Attribute read-side wait time to the
            // concrete lock name.
            scheduler::block_task_uninterruptible_named(
                waiter.as_ref().get_ref().task(),
                "sleep_rwlock_read",
                self.name,
            );
            waiters.push_back(waiter.as_mut());
        }

        while !waiter.as_ref().get_ref().is_granted() {
            current::schedule();
        }
        SleepRwLockOnStackReadGuard { lock: self }
    }

    pub fn write(&self) -> SleepRwLockOnStackWriteGuard<'_, T> {
        if self.try_write() {
            return SleepRwLockOnStackWriteGuard { lock: self };
        }

        let mut waiter = pin!(WaiterOnStack::new(current::task().clone(), WaitKind::Write));
        {
            let mut waiters = self.waiters.lock();
            loop {
                if self.try_write() {
                    return SleepRwLockOnStackWriteGuard { lock: self };
                }
                if self.mark_contended(WaitKind::Write) {
                    break;
                }
            }
            // PERF_DEBUG(scheduler-time): Attribute write-side wait time to the
            // concrete lock name.
            scheduler::block_task_uninterruptible_named(
                waiter.as_ref().get_ref().task(),
                "sleep_rwlock_write",
                self.name,
            );
            waiters.push_back(waiter.as_mut());
        }

        while !waiter.as_ref().get_ref().is_granted() {
            current::schedule();
        }
        SleepRwLockOnStackWriteGuard { lock: self }
    }

    fn try_read(&self) -> bool {
        let mut state = self.state.load(Ordering::Relaxed);
        loop {
            if state & (WRITER_LOCKED | CONTENDED) != 0 {
                return false;
            }
            if state & READER_MASK == READER_MASK {
                panic!("SleepRwLockOnStack reader count overflow");
            }
            match self
                .state
                .compare_exchange_weak(state, state + 1, Ordering::Acquire, Ordering::Relaxed)
            {
                Ok(_) => return true,
                Err(current) => state = current,
            }
        }
    }

    fn try_write(&self) -> bool {
        self.state
            .compare_exchange(0, WRITER_LOCKED, Ordering::Acquire, Ordering::Relaxed)
            .is_ok()
    }

    /// Marks the lock contended only while the requested ownership cannot be
    /// acquired. The caller must hold the waiter-list lock.
    fn mark_contended(&self, kind: WaitKind) -> bool {
        let mut state = self.state.load(Ordering::Relaxed);
        loop {
            if state & CONTENDED != 0 {
                return true;
            }
            let can_acquire = match kind {
                WaitKind::Read => state & WRITER_LOCKED == 0,
                WaitKind::Write => state == 0,
            };
            if can_acquire {
                return false;
            }
            match self
                .state
                .compare_exchange_weak(state, state | CONTENDED, Ordering::Relaxed, Ordering::Relaxed)
            {
                Ok(_) => return true,
                Err(current) => state = current,
            }
        }
    }

    fn read_unlock(&self) {
        let previous = self.state.fetch_sub(1, Ordering::Release);
        debug_assert_eq!(previous & WRITER_LOCKED, 0);
        debug_assert_ne!(previous & READER_MASK, 0);
        if previous & READER_MASK == 1 && previous & CONTENDED != 0 {
            // Synchronize with the release sequence formed by all preceding
            // reader unlocks before transferring ownership to a waiter.
            fence(Ordering::Acquire);
            self.grant_waiters();
        }
    }

    fn write_unlock(&self) {
        match self
            .state
            .compare_exchange(WRITER_LOCKED, 0, Ordering::Release, Ordering::Relaxed)
        {
            Ok(_) => {}
            Err(state) => {
                debug_assert_eq!(state, WRITER_LOCKED | CONTENDED);
                self.grant_waiters();
            }
        }
    }

    fn grant_waiters(&self) {
        let mut waiters = self.waiters.lock();
        let Some(front) = waiters.front() else {
            // A granted reader may drop after the complete reader batch has
            // been published but before the granting CPU clears CONTENDED.
            return;
        };
        // SAFETY: waiters are pinned while linked and the waiter-list spin lock
        // serializes access to the intrusive list.
        let front_kind = *unsafe { front.as_ref() }.value();
        match front_kind {
            WaitKind::Write => {
                let waiter = waiters.pop_front().unwrap();
                let next_state = WRITER_LOCKED | if waiters.front().is_some() { CONTENDED } else { 0 };
                self.state.store(next_state, Ordering::Release);
                // SAFETY: the waiter was pinned while linked and is removed
                // before its task is granted ownership and woken.
                let waiter = unsafe { waiter.as_ref() };
                waiter.grant();
                scheduler::wakeup_task_uninterruptible(waiter.task(), Event::SleepLock);
            }
            WaitKind::Read => {
                let reader_count = waiters.front_run_len(&WaitKind::Read);
                assert!(reader_count <= READER_MASK, "SleepRwLockOnStack reader count overflow");
                // Publish ownership for the whole reader batch before waking
                // any member. This prevents an early-finishing reader from
                // granting a following writer while later readers are still
                // being woken.
                self.state.store(reader_count | CONTENDED, Ordering::Release);
                for _ in 0..reader_count {
                    let waiter = waiters.pop_front().unwrap();
                    // SAFETY: the waiter was pinned while linked and is removed
                    // before its task is granted ownership and woken.
                    let waiter = unsafe { waiter.as_ref() };
                    waiter.grant();
                    scheduler::wakeup_task_uninterruptible(waiter.task(), Event::SleepLock);
                }
                if waiters.front().is_none() {
                    self.state.fetch_and(!CONTENDED, Ordering::Release);
                }
            }
        }
    }
}

// SAFETY: moving the lock is safe when its protected value is movable.
unsafe impl<T: Send> Send for SleepRwLockOnStack<T> {}

// SAFETY: readers may access `T` concurrently, so `T` must be `Sync`; writers
// are mutually exclusive and require `T: Send`.
unsafe impl<T: Send + Sync> Sync for SleepRwLockOnStack<T> {}
