#[cfg(feature = "no-smp")]
use core::cell::UnsafeCell;
use core::pin::pin;

#[cfg(not(feature = "no-smp"))]
use core::sync::atomic::Ordering;
use spin::mutex::SpinMutex;

use crate::kernel::event::Event;
use crate::kernel::scheduler::{self, current};

use super::locker::LockerTrait;
use super::mutex::{LockGuard, Mutex};
use super::onstack_waiter::{WaitList, WaiterOnStack};

pub struct SleepLockerOnStack {
    #[cfg(feature = "no-smp")]
    locked: UnsafeCell<bool>,
    #[cfg(not(feature = "no-smp"))]
    locked: core::sync::atomic::AtomicBool,
    waiters: SpinMutex<WaitList<()>>,
}

impl SleepLockerOnStack {
    pub const fn new() -> Self {
        Self {
            #[cfg(feature = "no-smp")]
            locked: UnsafeCell::new(false),
            #[cfg(not(feature = "no-smp"))]
            locked: core::sync::atomic::AtomicBool::new(false),
            waiters: SpinMutex::new(WaitList::new()),
        }
    }

    fn set_unlocked(&self) {
        #[cfg(feature = "no-smp")]
        {
            // SAFETY: no-smp guarantees that no other CPU can access the lock
            // state concurrently. Waiter changes are serialized separately.
            unsafe { *self.locked.get() = false };
        }
        #[cfg(not(feature = "no-smp"))]
        self.locked.store(false, Ordering::Release);
    }
}

impl LockerTrait for SleepLockerOnStack {
    fn try_lock(&self, _name: &'static str) -> bool {
        #[cfg(feature = "no-smp")]
        {
            // SAFETY: no-smp guarantees that only the current CPU can access
            // the lock state at this instant.
            unsafe {
                if *self.locked.get() {
                    false
                } else {
                    *self.locked.get() = true;
                    true
                }
            }
        }
        #[cfg(not(feature = "no-smp"))]
        self.locked
            .compare_exchange(false, true, Ordering::Acquire, Ordering::Relaxed)
            .is_ok()
    }

    fn lock(&self, name: &'static str) {
        let mut waiter = pin!(WaiterOnStack::new(current::task().clone(), ()));
        if self.try_lock(name) {
            return;
        }

        {
            let mut waiters = self.waiters.lock();
            if self.try_lock(name) {
                return;
            }
            // PERF_DEBUG(scheduler-time): Attribute on-stack sleep-lock wait
            // time to the concrete lock name.
            scheduler::block_task_uninterruptible_named(waiter.as_ref().get_ref().task(), "sleep_lock_onstack", name);
            waiters.push_back(waiter.as_mut());
        }

        while !waiter.as_ref().get_ref().is_granted() {
            current::schedule();
            let event = current::task().take_wakeup_event();
            #[cfg(feature = "debug-task")]
            match event {
                Some(Event::SleepLock) => {}
                Some(event) => crate::kwarn!(
                    "SleepLockOnStack '{}' waiter received unexpected event: {:?}",
                    name,
                    event
                ),
                None => crate::kwarn!("SleepLockOnStack '{}' waiter resumed without a wakeup event", name),
            }
            #[cfg(not(feature = "debug-task"))]
            let _ = event;
        }
    }

    fn unlock(&self, _name: &'static str) {
        let task = {
            let mut waiters = self.waiters.lock();
            if let Some(waiter) = waiters.pop_front() {
                // Keep the lock state locked while directly transferring
                // ownership to the first waiter.
                // SAFETY: the waiter was pinned while linked and remains alive
                // until its blocked task is woken below.
                let waiter = unsafe { waiter.as_ref() };
                waiter.grant();
                Some(waiter.task())
            } else {
                self.set_unlocked();
                None
            }
        };
        if let Some(task) = task {
            scheduler::wakeup_task_uninterruptible(task, Event::SleepLock);
        }
    }
}

// SAFETY: access to protected data is serialized by the lock state, and the
// waiter list is serialized by its spin lock.
unsafe impl Send for SleepLockerOnStack {}

// SAFETY: access to protected data is serialized by the lock state, and the
// waiter list is serialized by its spin lock.
unsafe impl Sync for SleepLockerOnStack {}

pub type SleepLockOnStack<T> = Mutex<T, SleepLockerOnStack>;
pub type SleepLockOnStackGuard<'a, T> = LockGuard<'a, T, SleepLockerOnStack>;

impl<T> SleepLockOnStack<T> {
    pub const fn new(data: T, name: &'static str) -> Self {
        Self::create(data, name, SleepLockerOnStack::new())
    }
}
