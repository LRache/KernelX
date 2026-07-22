use core::cell::UnsafeCell;
use core::ops::{Deref, DerefMut};

pub struct NoLockReadGuard<'a, T> {
    data: &'a T,
}

impl<T> Deref for NoLockReadGuard<'_, T> {
    type Target = T;
    fn deref(&self) -> &T {
        self.data
    }
}

pub struct NoLockGuard<'a, T> {
    data: &'a mut T,
}

impl<T> Deref for NoLockGuard<'_, T> {
    type Target = T;
    fn deref(&self) -> &T {
        self.data
    }
}

impl<T> DerefMut for NoLockGuard<'_, T> {
    fn deref_mut(&mut self) -> &mut T {
        self.data
    }
}

pub struct NoLockMutex<T> {
    data: UnsafeCell<T>,
}

impl<T> NoLockMutex<T> {
    pub const fn new(data: T, _name: &'static str) -> Self {
        NoLockMutex {
            data: UnsafeCell::new(data),
        }
    }

    pub fn lock(&self) -> NoLockGuard<'_, T> {
        NoLockGuard {
            data: unsafe { &mut *self.data.get() },
        }
    }

    /// Matches the normal lock API when lockdep is compiled out.
    ///
    /// # Safety
    ///
    /// The caller is responsible for the same invariants as `lock`.
    pub unsafe fn lock_unchecked(&self) -> NoLockGuard<'_, T> {
        self.lock()
    }
}

unsafe impl<T: Send> Send for NoLockMutex<T> {}
unsafe impl<T: Send> Sync for NoLockMutex<T> {}

pub struct NoLockRWLock<T> {
    data: UnsafeCell<T>,
}

impl<T> NoLockRWLock<T> {
    pub const fn new(data: T, _name: &'static str) -> Self {
        Self {
            data: UnsafeCell::new(data),
        }
    }

    pub fn read(&self) -> NoLockReadGuard<'_, T> {
        NoLockReadGuard {
            data: unsafe { &*self.data.get() },
        }
    }

    pub fn write(&self) -> NoLockGuard<'_, T> {
        NoLockGuard {
            data: unsafe { &mut *self.data.get() },
        }
    }
}

unsafe impl<T: Send> Send for NoLockRWLock<T> {}
unsafe impl<T: Send> Sync for NoLockRWLock<T> {}
