#[cfg(feature = "spinlock-check")]
use crate::kernel::scheduler::current;

use super::locker::LockerTrait;
use super::mutex::Mutex;

pub struct SpinLocker {
    #[cfg(feature = "no-smp")]
    lock: core::cell::UnsafeCell<bool>,

    #[cfg(not(feature = "no-smp"))]
    lock: core::sync::atomic::AtomicBool,
}

impl SpinLocker {
    pub const fn new() -> Self {
        SpinLocker {
            #[cfg(feature = "no-smp")]
            lock: core::cell::UnsafeCell::new(false),
            #[cfg(not(feature = "no-smp"))]
            lock: core::sync::atomic::AtomicBool::new(false),
        }
    }

    #[cfg(feature = "spinlock-check")]
    fn id(&self) -> usize {
        self as *const _ as usize
    }

    #[cfg(feature = "spinlock-check")]
    fn assert_not_held(&self, name: &'static str) {
        if current::has_processor() {
            current::processor().assert_not_holding_spinlock(self.id(), name);
        }
    }

    #[cfg(feature = "spinlock-check")]
    fn track_acquire(&self, name: &'static str) {
        if current::has_processor() {
            current::processor().acquire_spinlock(self.id(), name);
        }
    }

    #[cfg(feature = "spinlock-check")]
    fn track_release(&self, name: &'static str) {
        if current::has_processor() {
            current::processor().release_spinlock(self.id(), name);
        }
    }
}

impl LockerTrait for SpinLocker {
    fn try_lock(&self, name: &'static str) -> bool {
        let _ = name;
        #[cfg(feature = "spinlock-check")]
        self.assert_not_held(name);

        #[cfg(feature = "no-smp")]
        unsafe {
            if *self.lock.get() {
                false
            } else {
                *self.lock.get() = true;
                #[cfg(feature = "spinlock-check")]
                self.track_acquire(name);
                true
            }
        }
        #[cfg(not(feature = "no-smp"))]
        {
            let acquired = self
                .lock
                .compare_exchange(
                    false,
                    true,
                    core::sync::atomic::Ordering::Acquire,
                    core::sync::atomic::Ordering::Relaxed,
                )
                .is_ok();
            #[cfg(feature = "spinlock-check")]
            if acquired {
                self.track_acquire(name);
            }
            acquired
        }
    }

    fn spin(&self) {}

    fn lock(&self, name: &'static str) {
        let _ = name;
        #[cfg(feature = "spinlock-check")]
        self.assert_not_held(name);

        #[cfg(feature = "no-smp")]
        unsafe {
            if *self.lock.get() {
                panic!(
                    "Deadlock detected in SpinLocker: single-core system cannot re-lock an already locked SpinLocker"
                );
            }
            *self.lock.get() = true;
        }
        #[cfg(not(feature = "no-smp"))]
        while self
            .lock
            .compare_exchange_weak(
                false,
                true,
                core::sync::atomic::Ordering::Acquire,
                core::sync::atomic::Ordering::Relaxed,
            )
            .is_err()
        {}
        #[cfg(feature = "spinlock-check")]
        self.track_acquire(name);
    }

    fn unlock(&self, name: &'static str) {
        let _ = name;
        #[cfg(feature = "no-smp")]
        unsafe {
            *self.lock.get() = false;
        }
        #[cfg(not(feature = "no-smp"))]
        self.lock.store(false, core::sync::atomic::Ordering::Release);
        #[cfg(feature = "spinlock-check")]
        self.track_release(name);
    }

    fn is_locked(&self) -> bool {
        #[cfg(feature = "no-smp")]
        unsafe {
            *self.lock.get()
        }
        #[cfg(not(feature = "no-smp"))]
        self.lock.load(core::sync::atomic::Ordering::Acquire)
    }
}

pub type SpinLock<T> = Mutex<T, SpinLocker>;

impl<T> SpinLock<T> {
    pub const fn new(data: T, name: &'static str) -> Self {
        Self::create(data, name, SpinLocker::new())
    }
}
