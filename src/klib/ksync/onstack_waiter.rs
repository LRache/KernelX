use alloc::sync::Arc;
use core::cell::UnsafeCell;
use core::marker::PhantomPinned;
use core::pin::Pin;
use core::ptr::NonNull;
use core::sync::atomic::{AtomicBool, Ordering};

use crate::kernel::scheduler::Task;

pub(super) struct WaiterOnStack<T> {
    task: Arc<dyn Task>,
    value: T,
    granted: AtomicBool,
    next: UnsafeCell<Option<NonNull<WaiterOnStack<T>>>>,
    _pin: PhantomPinned,
}

impl<T> WaiterOnStack<T> {
    pub(super) fn new(task: Arc<dyn Task>, value: T) -> Self {
        Self {
            task,
            value,
            granted: AtomicBool::new(false),
            next: UnsafeCell::new(None),
            _pin: PhantomPinned,
        }
    }

    pub(super) fn task(&self) -> Arc<dyn Task> {
        self.task.clone()
    }

    pub(super) fn value(&self) -> &T {
        &self.value
    }

    pub(super) fn grant(&self) {
        self.granted.store(true, Ordering::Release);
    }

    pub(super) fn is_granted(&self) -> bool {
        self.granted.load(Ordering::Acquire)
    }

    fn next_mut(&self) -> &mut Option<NonNull<Self>> {
        // SAFETY: access to `next` is private to this module and serialized by
        // the spin lock protecting the wait list. A linked waiter remains
        // pinned in the blocked task's stack frame until it is removed.
        unsafe { &mut *self.next.get() }
    }

    fn next(&self) -> Option<NonNull<Self>> {
        // SAFETY: access to `next` is private to this module and serialized by
        // the spin lock protecting the wait list.
        unsafe { *self.next.get() }
    }
}

pub(super) struct WaitList<T> {
    head: Option<NonNull<WaiterOnStack<T>>>,
    tail: Option<NonNull<WaiterOnStack<T>>>,
}

impl<T> WaitList<T> {
    pub(super) const fn new() -> Self {
        Self { head: None, tail: None }
    }

    pub(super) fn push_back(&mut self, waiter: Pin<&mut WaiterOnStack<T>>) {
        let waiter = NonNull::from(waiter.as_ref().get_ref());
        #[cfg(debug_assertions)]
        {
            let mut current = self.head;
            while let Some(current_waiter) = current {
                assert_ne!(current_waiter, waiter, "waiter is already linked");
                // SAFETY: all linked waiters remain pinned, and the caller
                // holds the spin lock protecting this list.
                current = *unsafe { current_waiter.as_ref() }.next_mut();
            }

            // SAFETY: the waiter is pinned and not linked in another list.
            debug_assert!(unsafe { waiter.as_ref() }.next_mut().is_none());
        }

        if let Some(tail) = self.tail {
            // SAFETY: the tail points to a pinned waiter, and list mutation is
            // serialized by the caller's spin lock.
            *unsafe { tail.as_ref() }.next_mut() = Some(waiter);
        } else {
            self.head = Some(waiter);
        }
        self.tail = Some(waiter);
    }

    pub(super) fn front(&self) -> Option<NonNull<WaiterOnStack<T>>> {
        self.head
    }

    pub(super) fn front_run_len(&self, value: &T) -> usize
    where
        T: PartialEq,
    {
        let mut len = 0;
        let mut current = self.head;
        while let Some(waiter) = current {
            // SAFETY: all linked waiters remain pinned, and the caller holds
            // the spin lock protecting this list.
            let waiter = unsafe { waiter.as_ref() };
            if waiter.value() != value {
                break;
            }
            len += 1;
            current = waiter.next();
        }
        len
    }

    pub(super) fn pop_front(&mut self) -> Option<NonNull<WaiterOnStack<T>>> {
        let waiter = self.head?;
        // SAFETY: the head points to a pinned waiter, and list mutation is
        // serialized by the caller's spin lock.
        let next = unsafe { waiter.as_ref() }.next_mut().take();
        self.head = next;
        if next.is_none() {
            self.tail = None;
        }
        Some(waiter)
    }
}

// SAFETY: raw waiter pointers are only accessed while holding the spin lock
// protecting the list. Every linked waiter is pinned in a blocked task's stack
// frame and is removed before that task is woken.
unsafe impl<T: Send + Sync> Send for WaitList<T> {}
