use core::cell::UnsafeCell;
use core::ops::{Deref, DerefMut};
use core::sync::atomic::AtomicBool;

use crate::kernel::scheduler::current;
use crate::kernel::task::Tid;

pub trait LockTrait {
    fn is_locked(&self) -> bool;
    fn lock(&self);
    fn unlock(&self);
}

pub struct LockGuard<'a, T, R: LockTrait> {
    data: &'a mut T,
    mutex: &'a Mutex<T, R>,
}

impl<T, R: LockTrait> Deref for LockGuard<'_, T, R> {
    type Target = T;
    fn deref(&self) -> &Self::Target {
        self.data
    }
}

impl<T, R: LockTrait> DerefMut for LockGuard<'_, T, R> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        self.data
    }
}

impl<T, R: LockTrait> Drop for LockGuard<'_, T, R> {
    fn drop(&mut self) {
        self.mutex.unlock();
    }
}

pub struct Mutex<T, R: LockTrait> {
    data: UnsafeCell<T>,
    lock: R,
    holder: Tid,
}

impl<T, R: LockTrait> Mutex<T, R> {
    pub fn lock(&self) -> LockGuard<'_, T, R> {
        #[cfg(feature = "deadlock-detect")]
        if self.lock.is_locked() {
            let tid = current::tid();
            if tid >= 0 && self.holder == tid {
                panic!("Deadlock detected in Mutex: current thread {} is trying to lock a mutex it already holds", tid);
            }
        }
        
        self.lock.lock();
        LockGuard {
            data: unsafe { &mut *self.data.get() },
            mutex: self,
        }
    }

    fn unlock(&self) {
        self.lock.unlock();
    }
}

unsafe impl<T: Send, R: LockTrait + Send> Send for Mutex<T, R> {}
unsafe impl<T: Send, R: LockTrait + Send> Sync for Mutex<T, R> {}

pub struct SpinLock {
    lock: AtomicBool,
}

impl SpinLock {
    pub const fn new() -> Self {
        SpinLock {
            lock: AtomicBool::new(false),
        }
    }
}

impl LockTrait for SpinLock {
    fn is_locked(&self) -> bool {
        self.lock.load(core::sync::atomic::Ordering::Relaxed)
    }
    
    fn lock(&self) {
        while self.lock.compare_exchange_weak(false, true, core::sync::atomic::Ordering::Acquire, core::sync::atomic::Ordering::Relaxed).is_err() {}
    }

    fn unlock(&self) {
        self.lock.store(false, core::sync::atomic::Ordering::Release);
    }
}

pub type SpinMutex<T> = Mutex<T, SpinLock>;

impl<T> SpinMutex<T> {
    pub const fn new(data: T) -> Self {
        SpinMutex {
            data: UnsafeCell::new(data),
            lock: SpinLock::new(),
            holder: -1,
        }
    }
}
