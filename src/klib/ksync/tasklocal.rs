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

    fn assert_current(&self) {
        assert!(self.tid == current::tid(), "TaskLocal accessed from non-owner task");
    }

    pub fn get_mut(&self) -> &mut T {
        self.assert_current();
        unsafe { &mut *self.value.get() }
    }

    pub fn set(&self, value: T) {
        self.assert_current();
        unsafe { *self.value.get() = value };
    }
}

impl<T> Deref for TaskLocal<T> {
    type Target = T;

    fn deref(&self) -> &Self::Target {
        self.assert_current();
        unsafe { &*self.value.get() }
    }
}

impl<T> DerefMut for TaskLocal<T> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        self.assert_current();
        unsafe { &mut *self.value.get() }
    }
}
