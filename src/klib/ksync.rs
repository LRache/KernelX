use core::cell::UnsafeCell;
use core::ops::{Deref, DerefMut};

#[cfg(not(feature = "no-smp"))]
use core::sync::atomic::AtomicBool;

#[cfg(feature = "no-smp")]
use core::sync::atomic::AtomicBool;

use crate::kernel::scheduler::{current, Tid};

pub trait LockerTrait {
    fn is_locked(&self) -> bool;
    fn lock(&self);
    fn unlock(&self);
}

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
    lock: R,
    holder: UnsafeCell<Tid>,
}

impl<T, R: LockerTrait> Mutex<T, R> {
    pub fn deadlock_detect(&self) -> bool {
        if self.lock.is_locked() {
            let tid = current::tid();
            if tid >= 0 && self.holder() == tid {
                return true;
            }
        }
        return false;
    }

    pub fn lock(&self) -> LockGuard<'_, T, R> {
        let tid = current::tid();
        #[cfg(feature = "deadlock-detect")]
        if self.deadlock_detect() {
            panic!("Deadlock detected in Mutex: current thread {} is trying to lock a mutex it already holds", tid);
        }
        
        self.lock.lock();
        self.set_holder(tid);
        
        LockGuard {
            data: unsafe { &mut *self.data.get() },
            mutex: self,
        }
    }

    fn unlock(&self) {
        self.lock.unlock();
    }

    fn holder(&self) -> Tid {
        unsafe { *self.holder.get() }
    }

    fn set_holder(&self, tid: Tid) {
        unsafe {
            *self.holder.get() = tid;
        }
    }
}

unsafe impl<T: Send, R: LockerTrait + Send> Send for Mutex<T, R> {}
unsafe impl<T: Send, R: LockerTrait + Send> Sync for Mutex<T, R> {}

pub struct SpinLocker {
    #[cfg(feature = "no-smp")]
    lock: UnsafeCell<bool>,

    #[cfg(not(feature = "no-smp"))]
    lock: AtomicBool,
}

impl SpinLocker {
    pub const fn new() -> Self {
        SpinLocker {
            #[cfg(feature = "no-smp")]
            lock: UnsafeCell::new(false),

            #[cfg(not(feature = "no-smp"))]
            lock: AtomicBool::new(false),
        }
    }
}

impl LockerTrait for SpinLocker {
    fn is_locked(&self) -> bool {
        #[cfg(feature = "no-smp")]
        { unsafe { *self.lock.get() } }
        #[cfg(not(feature = "no-smp"))]
        self.lock.load(core::sync::atomic::Ordering::Relaxed)
    }
    
    fn lock(&self) {
        #[cfg(feature = "no-smp")]
        unsafe {
            if *self.lock.get() {
                panic!("Deadlock detected in SpinLocker: single-core system cannot re-lock an already locked SpinLocker");
            }
            *self.lock.get() = true;
        }
        #[cfg(not(feature = "no-smp"))]
        while self.lock.compare_exchange_weak(false, true, core::sync::atomic::Ordering::Acquire, core::sync::atomic::Ordering::Relaxed).is_err() {}
    }

    fn unlock(&self) {
        #[cfg(feature = "no-smp")]
        unsafe {
            *self.lock.get() = false;
        }
        #[cfg(not(feature = "no-smp"))]
        self.lock.store(false, core::sync::atomic::Ordering::Release);
    }
}

pub type SpinLock<T> = Mutex<T, SpinLocker>;

impl<T> SpinLock<T> {
    pub const fn new(data: T) -> Self {
        SpinLock {
            data: UnsafeCell::new(data),
            lock: SpinLocker::new(),
            holder: UnsafeCell::new(-1),
        }
    }

    #[cfg(not(feature = "no-smp"))]
    pub fn is_locked(&self) -> bool {
        self.lock.is_locked()
    }
}

use alloc::collections::VecDeque;
use alloc::sync::Arc;
use crate::kernel::event::Event;
use crate::kernel::scheduler::{self, Task};

pub struct SleepLocker {
    locked: AtomicBool,
    waiters: SpinLock<VecDeque<Arc<dyn Task>>>,
}

impl SleepLocker {
    pub const fn new() -> Self {
        SleepLocker {
            locked: AtomicBool::new(false),
            waiters: SpinLock::new(VecDeque::new()),
        }
    }
}

impl LockerTrait for SleepLocker {
    fn is_locked(&self) -> bool {
        self.locked.load(core::sync::atomic::Ordering::Relaxed)
    }

    fn lock(&self) {
        use core::sync::atomic::Ordering;
        loop {
            if self.locked.compare_exchange(false, true, Ordering::Acquire, Ordering::Relaxed).is_ok() {
                return;
            }
            let task = current::task().clone();
            task.block("sleep_lock");
            self.waiters.lock().push_back(task);
            // Re-check after enqueue: if lock was released between CAS fail and enqueue,
            // the unlocker may not have seen us. Try to acquire before sleeping.
            if self.locked.compare_exchange(false, true, Ordering::Acquire, Ordering::Relaxed).is_ok() {
                // Got the lock — remove ourselves from waiters and wake up
                let mut waiters = self.waiters.lock();
                if let Some(pos) = waiters.iter().position(|t| Arc::ptr_eq(t, current::task())) {
                    waiters.remove(pos);
                }
                // Unblock ourselves since we won't actually sleep
                scheduler::wakeup_task(current::task().clone(), Event::ReadReady);
                return;
            }
            current::schedule();
        }
    }

    fn unlock(&self) {
        self.locked.store(false, core::sync::atomic::Ordering::Release);
        let mut waiters = self.waiters.lock();
        if let Some(task) = waiters.pop_front() {
            scheduler::wakeup_task(task, Event::ReadReady);
        }
    }
}

unsafe impl Send for SleepLocker {}
unsafe impl Sync for SleepLocker {}

pub type SleepLock<T> = Mutex<T, SleepLocker>;

impl<T> SleepLock<T> {
    pub const fn new(data: T) -> Self {
        SleepLock {
            data: UnsafeCell::new(data),
            lock: SleepLocker::new(),
            holder: UnsafeCell::new(-1),
        }
    }
}

#[macro_export]
macro_rules! lock_debug {
    ($mutex:expr) => {{
        let mutex = &$mutex;
        let current_tid = $crate::kernel::scheduler::current::tid();
        
        if mutex.deadlock_detect() {
            panic!(
                "Deadlock detected in Mutex: current thread {} is trying to lock a mutex it already holds",
                current_tid,
            );
        }
        
        mutex.lock()
    }};
}
