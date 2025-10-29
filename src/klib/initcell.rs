use core::cell::UnsafeCell;
use core::mem::MaybeUninit;
use core::ops::Deref;
use core::sync::atomic::{AtomicBool, Ordering};

pub struct InitedCell<T> {
    value: UnsafeCell<MaybeUninit<T>>,
    #[cfg(debug_assertions)]
    is_inited: AtomicBool,
}

impl<T> InitedCell<T> {
    pub const fn uninit() -> Self {
        Self {
            value: UnsafeCell::new(MaybeUninit::uninit()),
            is_inited: AtomicBool::new(false)
        }
    }

    pub fn init(&self, value: T) {
        debug_assert!(self.is_inited.compare_exchange(false, true, Ordering::AcqRel, Ordering::Relaxed).is_ok(), "InitedCell has already been initialized.");

        unsafe {
            *self.value.get() = MaybeUninit::new(value);
        }
    }
}

impl<T> Deref for InitedCell<T> {
    type Target = T;

    #[inline]
    fn deref(&self) -> &Self::Target {
        debug_assert!(self.is_inited.load(Ordering::Relaxed), "Cannot access uninitialized InitedCell.");
        unsafe {
            (*self.value.get()).assume_init_ref()
        }
    }
}

unsafe impl<T> Sync for InitedCell<T> {}
