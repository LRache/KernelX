use core::cell::UnsafeCell;
use core::mem::MaybeUninit;
use core::ops::{Deref, DerefMut};
use core::ptr;

pub struct InitedCell<T> {
    value: UnsafeCell<MaybeUninit<T>>,
    is_inited: UnsafeCell<bool>,
}

impl<T> InitedCell<T> {
    pub const fn new() -> Self {
        Self {
            value: UnsafeCell::new(MaybeUninit::uninit()),
            is_inited: UnsafeCell::new(false),
        }
    }

    pub fn init(&self, value: T) {
        debug_assert!(!self.is_inited(), "InitedCell has already been initialized.");

        unsafe {
            *self.value.get() = MaybeUninit::new(value);
            *self.is_inited.get() = true;
        }
    }

    #[inline]
    pub fn is_inited(&self) -> bool {
        unsafe { *self.is_inited.get() }
    }

    #[inline]
    pub fn get(&self) -> &T {
        debug_assert!(self.is_inited(), "Cannot access uninitialized InitedCell.");
        unsafe {
            (*self.value.get()).assume_init_ref()
        }
    }
    
    #[inline]
    pub fn get_mut(&self) -> &mut T {
        debug_assert!(self.is_inited(), "Cannot access uninitialized InitedCell.");
        unsafe {
            (*self.value.get()).assume_init_mut()
        }
    }
}

impl<T> Deref for InitedCell<T> {
    type Target = T;

    #[inline]
    fn deref(&self) -> &Self::Target {
        self.get()
    }
}

impl<T> DerefMut for InitedCell<T> {
    #[inline]
    fn deref_mut(&mut self) -> &mut Self::Target {
        self.get_mut()
    }
}

impl<T> Drop for InitedCell<T> {
    fn drop(&mut self) {
        if self.is_inited() {
            unsafe {
                ptr::drop_in_place((*self.value.get()).as_mut_ptr());
            }
        }
    }
}

unsafe impl<T> Sync for InitedCell<T> {}
