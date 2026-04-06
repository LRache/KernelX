use core::cell::UnsafeCell;
use core::ops::{Deref, DerefMut};

use crate::kernel::scheduler::{Tid, current};

pub struct TaskLocal<T> {
    tid: Tid,
    value: UnsafeCell<T>,
}

impl<T> TaskLocal<T> {
    pub fn new(tid: Tid, value: T) -> Self {
        TaskLocal {
            tid,
            value: UnsafeCell::new(value),
        }
    }

    pub fn get_mut(&self) -> &mut T {
        debug_assert!(self.tid == current::tid());
        unsafe { &mut *self.value.get() }
    }

    pub fn set(&self, value: T) {
        debug_assert!(self.tid == current::tid());
        unsafe { *self.value.get() = value };
    }
}

impl<T> Deref for TaskLocal<T> {
    type Target = T;

    fn deref(&self) -> &Self::Target {
        debug_assert!(self.tid == current::tid());
        unsafe { &*self.value.get() }
    }
}

impl<T> DerefMut for TaskLocal<T> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        debug_assert!(self.tid == current::tid());
        unsafe { &mut *self.value.get() }
    }
}
