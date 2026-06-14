use core::cell::UnsafeCell;
use core::mem::MaybeUninit;
use core::ops::Deref;
use core::sync::atomic::{AtomicU8, Ordering};
#[repr(u8)]
enum InitState {
    Uninit = 0,
    Initializing = 1,
    Inited = 2,
}

pub struct InitedCell<T> {
    value: UnsafeCell<MaybeUninit<T>>,
    state: AtomicU8,
}

impl<T> InitedCell<T> {
    pub const fn uninit() -> Self {
        Self {
            value: UnsafeCell::new(MaybeUninit::uninit()),
            state: AtomicU8::new(InitState::Uninit as u8),
        }
    }

    pub fn init(&self, value: T) {
        assert!(
            self.state
                .compare_exchange(
                    InitState::Uninit as u8,
                    InitState::Initializing as u8,
                    Ordering::AcqRel,
                    Ordering::Relaxed,
                )
                .is_ok(),
            "InitedCell has already been initialized."
        );

        unsafe {
            (*self.value.get()).write(value);
        }
        self.state.store(InitState::Inited as u8, Ordering::Release);
    }

    pub fn try_get(&self) -> Option<&T> {
        if self.state.load(Ordering::Acquire) == InitState::Inited as u8 {
            Some(unsafe { (*self.value.get()).assume_init_ref() })
        } else {
            None
        }
    }
}

impl<T> Deref for InitedCell<T> {
    type Target = T;

    #[inline]
    fn deref(&self) -> &Self::Target {
        assert!(
            self.state.load(Ordering::Acquire) == InitState::Inited as u8,
            "Cannot access uninitialized InitedCell."
        );
        unsafe { (*self.value.get()).assume_init_ref() }
    }
}

unsafe impl<T: Sync> Sync for InitedCell<T> {}
