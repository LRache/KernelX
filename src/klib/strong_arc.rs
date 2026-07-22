use alloc::boxed::Box;
use core::ops::Deref;
use core::ptr::NonNull;
use core::sync::atomic::{AtomicUsize, Ordering, fence};

struct StrongArcInner<T> {
    strong: AtomicUsize,
    value: T,
}

/// An atomically reference-counted pointer without weak references.
///
/// `StrongArc` is intended for shared ownership graphs where reference cycles
/// are ruled out by design. The allocation is released as soon as the last
/// strong pointer is dropped.
pub struct StrongArc<T> {
    inner: NonNull<StrongArcInner<T>>,
}

impl<T> StrongArc<T> {
    pub fn new(value: T) -> Self {
        let inner = Box::new(StrongArcInner {
            strong: AtomicUsize::new(1),
            value,
        });
        Self {
            inner: NonNull::from(Box::leak(inner)),
        }
    }

    pub fn get_mut(this: &mut Self) -> Option<&mut T> {
        if Self::strong_count(this) == 1 {
            // SAFETY: A strong count of one means no other `StrongArc` can
            // access the allocation, and `this` is borrowed exclusively.
            Some(unsafe { &mut this.inner.as_mut().value })
        } else {
            None
        }
    }

    pub fn strong_count(this: &Self) -> usize {
        // SAFETY: Every `StrongArc` owns a strong reference, so the allocation
        // remains valid for the duration of this borrow.
        unsafe { this.inner.as_ref().strong.load(Ordering::Relaxed) }
    }

    pub fn ptr_eq(this: &Self, other: &Self) -> bool {
        this.inner == other.inner
    }
}

impl<T> Clone for StrongArc<T> {
    fn clone(&self) -> Self {
        // SAFETY: `self` owns a strong reference, so the allocation is valid.
        let result = unsafe {
            self.inner
                .as_ref()
                .strong
                .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |count| {
                    (count < isize::MAX as usize).then_some(count + 1)
                })
        };
        assert!(result.is_ok(), "StrongArc reference count overflow");
        Self { inner: self.inner }
    }
}

impl<T> Deref for StrongArc<T> {
    type Target = T;

    fn deref(&self) -> &Self::Target {
        // SAFETY: Every `StrongArc` owns a strong reference, so the allocation
        // remains valid for the duration of this borrow.
        unsafe { &self.inner.as_ref().value }
    }
}

impl<T> Drop for StrongArc<T> {
    fn drop(&mut self) {
        // SAFETY: `self` owns a strong reference, so the allocation is valid.
        let inner = unsafe { self.inner.as_ref() };
        if inner.strong.fetch_sub(1, Ordering::Release) != 1 {
            return;
        }

        fence(Ordering::Acquire);
        // SAFETY: The decrement above removed the last strong reference. No
        // other pointer can access the allocation, which came from `Box`.
        drop(unsafe { Box::from_raw(self.inner.as_ptr()) });
    }
}

// SAFETY: Shared access through `StrongArc` is safe when `T` can be shared and
// transferred between threads.
unsafe impl<T: Send + Sync> Send for StrongArc<T> {}

// SAFETY: Shared access through `StrongArc` is safe when `T` can be shared and
// transferred between threads.
unsafe impl<T: Send + Sync> Sync for StrongArc<T> {}
