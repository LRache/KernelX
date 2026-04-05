use core::cell::UnsafeCell;
use core::ops::{Deref, DerefMut};

#[cfg(feature = "deadlock-detect")]
use core::sync::atomic::{AtomicI32, Ordering};
#[cfg(all(feature = "deadlock-detect", feature = "backtrace"))]
use spin::mutex::SpinMutex;

#[cfg(feature = "deadlock-detect")]
use crate::kernel::scheduler::current;
#[cfg(feature = "deadlock-detect")]
use crate::klib::backtrace;

use super::locker::LockerTrait;

pub struct LockGuard<'a, T, R: LockerTrait> {
    data: &'a mut T,
    mutex: &'a Mutex<T, R>,
}

impl<T, R: LockerTrait> Deref for LockGuard<'_, T, R> {
    type Target = T;
    fn deref(&self) -> &Self::Target {
        self.data
    }
}

impl<T, R: LockerTrait> DerefMut for LockGuard<'_, T, R> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        self.data
    }
}

impl<T, R: LockerTrait> Drop for LockGuard<'_, T, R> {
    fn drop(&mut self) {
        self.mutex.unlock();
    }
}

pub struct Mutex<T, R: LockerTrait> {
    data: UnsafeCell<T>,
    locker: R,

    #[cfg(feature = "deadlock-detect")]
    name: &'static str,

    /// TID of the task currently holding this lock (-1 = unlocked).
    #[cfg(feature = "deadlock-detect")]
    holder_tid: AtomicI32,

    /// Backtrace captured at the moment this lock was last acquired.
    #[cfg(all(feature = "deadlock-detect", feature = "backtrace"))]
    acquire_bt: SpinMutex<Option<alloc::collections::LinkedList<usize>>>,
}

impl<T, R: LockerTrait> Mutex<T, R> {
    pub const fn create(data: T, name: &'static str, locker: R) -> Self {
        let _ = name;
        Mutex {
            data: UnsafeCell::new(data),
            locker,

            #[cfg(feature = "deadlock-detect")]
            name,

            #[cfg(feature = "deadlock-detect")]
            holder_tid: AtomicI32::new(-1),

            #[cfg(all(feature = "deadlock-detect", feature = "backtrace"))]
            acquire_bt: SpinMutex::new(None),
        }
    }

    fn name(&self) -> &'static str {
        #[cfg(feature = "deadlock-detect")]
        {
            self.name
        }
        #[cfg(not(feature = "deadlock-detect"))]
        {
            ""
        }
    }

    pub fn lock(&self) -> LockGuard<'_, T, R> {
        #[cfg(feature = "deadlock-detect")]
        if current::has_task() {
            use crate::klib::ksync::lockdep;

            let task = current::task();
            lockdep::on_acquire(task.lockstate().held(), self.name);
        }

        if !self.locker.try_lock(&self.name()) {
            #[cfg(all(feature = "deadlock-detect", feature = "backtrace"))]
            if current::has_task() {
                current::task()
                    .lockstate()
                    .set_waiting(Some((self.name(), self.acquire_bt.lock().clone().unwrap())));
            }
            while !self.locker.try_lock(&self.name()) {
                self.locker.spin();
            }
        }

        // self.locker.lock(self.name);

        #[cfg(feature = "deadlock-detect")]
        if current::has_task() {
            self.holder_tid.store(current::tid(), Ordering::Relaxed);
            let lockstate = current::task().lockstate();
            lockstate.acquire(self.name);
            lockstate.set_waiting(None);
        }

        #[cfg(all(feature = "deadlock-detect", feature = "backtrace"))]
        {
            *self.acquire_bt.lock() = Some(backtrace::backtrace_chain());
        }

        LockGuard {
            data: unsafe { &mut *self.data.get() },
            mutex: self,
        }
    }

    fn unlock(&self) {
        #[cfg(feature = "deadlock-detect")]
        if current::has_task() {
            current::task().lockstate().release(self.name);
            self.holder_tid.store(-1, Ordering::Relaxed);
        }

        #[cfg(all(feature = "deadlock-detect", feature = "backtrace"))]
        {
            *self.acquire_bt.lock() = None;
        }

        self.locker.unlock(self.name());
    }

    pub fn is_locked(&self) -> bool {
        self.locker.is_locked()
    }

    /// Returns the TID of the task currently holding this lock, or -1 if free.
    #[cfg(feature = "deadlock-detect")]
    pub fn holder_tid(&self) -> i32 {
        self.holder_tid.load(Ordering::Relaxed)
    }
}

unsafe impl<T: Send, R: LockerTrait + Send> Send for Mutex<T, R> {}
unsafe impl<T: Send, R: LockerTrait + Send> Sync for Mutex<T, R> {}
