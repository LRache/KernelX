use core::cell::UnsafeCell;
use core::ops::{Deref, DerefMut};

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

    pub fn is_locked(&self) -> bool {
        false
    }
}

unsafe impl<T: Send> Send for NoLockMutex<T> {}
unsafe impl<T: Send> Sync for NoLockMutex<T> {}
