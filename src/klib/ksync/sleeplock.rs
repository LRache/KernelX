use alloc::collections::VecDeque;
use alloc::sync::Arc;
#[cfg(feature = "no-smp")]
use core::cell::UnsafeCell;
#[cfg(not(feature = "no-smp"))]
use core::sync::atomic::Ordering;
use spin::mutex::SpinMutex;

use crate::kernel::event::Event;
use crate::kernel::scheduler::{self, Task, current};

use super::locker::LockerTrait;
use super::mutex::Mutex;

pub struct SleepLocker {
    #[cfg(feature = "no-smp")]
    locked: core::cell::UnsafeCell<bool>,
    #[cfg(not(feature = "no-smp"))]
    locked: core::sync::atomic::AtomicBool,
    waiters: SpinMutex<VecDeque<Arc<dyn Task>>>,
}

impl SleepLocker {
    pub const fn new() -> Self {
        SleepLocker {
            #[cfg(feature = "no-smp")]
            locked: UnsafeCell::new(false),
            #[cfg(not(feature = "no-smp"))]
            locked: core::sync::atomic::AtomicBool::new(false),
            waiters: SpinMutex::new(VecDeque::new()),
        }
    }
}

impl LockerTrait for SleepLocker {
    fn try_lock(&self, _name: &'static str) -> bool {
        #[cfg(feature = "no-smp")]
        unsafe {
            if *self.locked.get() {
                false
            } else {
                *self.locked.get() = true;
                true
            }
        }
        #[cfg(not(feature = "no-smp"))]
        self.locked
            .compare_exchange(false, true, Ordering::Acquire, Ordering::Relaxed)
            .is_ok()
    }

    fn spin(&self) {
        let task = current::task();

        {
            let mut waiters = self.waiters.lock();
            waiters.push_back(task.clone());
        }

        scheduler::block_task_uninterruptible(task.clone(), "sleep_lock");
        current::schedule();
    }

    fn lock(&self, name: &'static str) {
        loop {
            if self.try_lock(name) {
                return;
            }

            let task = current::task();

            // Hold `waiters` lock while doing the second try_lock() check AND
            // adding ourselves to the waiters list. `unlock()` also holds
            // `waiters` lock after setting locked=false, so these two critical
            // sections are mutually exclusive — preventing lost wakeups.
            {
                let mut waiters = self.waiters.lock();
                // Re-check: if unlock() ran between the first try_lock() above
                // and here, we will see locked==false now and take the lock
                // without sleeping.
                if self.try_lock(name) {
                    return;
                }
                scheduler::block_task_uninterruptible(task.clone(), "sleep_lock");
                waiters.push_back(task.clone());
            }

            current::schedule();
        }
    }

    fn unlock(&self, _name: &'static str) {
        #[cfg(feature = "no-smp")]
        unsafe {
            *self.locked.get() = false;
        }
        #[cfg(not(feature = "no-smp"))]
        self.locked.store(false, core::sync::atomic::Ordering::Release);

        let mut waiters = self.waiters.lock();
        if let Some(task) = waiters.pop_front() {
            scheduler::wakeup_task_uninterruptible(task, Event::SleepLock);
        }
    }

    fn is_locked(&self) -> bool {
        #[cfg(feature = "no-smp")]
        unsafe {
            *self.locked.get()
        }
        #[cfg(not(feature = "no-smp"))]
        self.locked.load(Ordering::Acquire)
    }
}

unsafe impl Send for SleepLocker {}
unsafe impl Sync for SleepLocker {}

pub type SleepLock<T> = Mutex<T, SleepLocker>;

impl<T> SleepLock<T> {
    pub const fn new(data: T, name: &'static str) -> Self {
        Self::create(data, name, SleepLocker::new())
    }
}
