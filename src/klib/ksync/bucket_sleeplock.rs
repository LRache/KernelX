use alloc::sync::Arc;
use core::cell::UnsafeCell;
use core::marker::PhantomPinned;
use core::pin::{Pin, pin};
use core::ptr::NonNull;

#[cfg(not(feature = "no-smp"))]
use core::sync::atomic::Ordering;
use spin::mutex::SpinMutex;

use crate::kernel::event::Event;
use crate::kernel::scheduler::{self, Task, current};

use super::locker::LockerTrait;
use super::mutex::{LockGuard, Mutex};

const WAIT_BUCKET_COUNT: usize = 64;

struct WaiterOnStack {
    lock_key: usize,
    task: Arc<dyn Task>,
    next: UnsafeCell<Option<NonNull<WaiterOnStack>>>,
    _pin: PhantomPinned,
}

impl WaiterOnStack {
    fn new(lock_key: usize, task: Arc<dyn Task>) -> Self {
        Self {
            lock_key,
            task,
            next: UnsafeCell::new(None),
            _pin: PhantomPinned,
        }
    }

    fn next_mut(&self) -> &mut Option<NonNull<Self>> {
        // SAFETY: next access is private to this module and serialized by the
        // hash bucket containing this waiter. The waiter remains pinned in the
        // blocked task's lock() stack frame while linked.
        unsafe { &mut *self.next.get() }
    }
}

struct BucketSleepLockWaitList {
    head: Option<NonNull<WaiterOnStack>>,
    tail: Option<NonNull<WaiterOnStack>>,
}

impl BucketSleepLockWaitList {
    const fn new() -> Self {
        Self { head: None, tail: None }
    }

    fn push_back(&mut self, waiter: Pin<&mut WaiterOnStack>) {
        let waiter = NonNull::from(waiter.as_ref().get_ref());
        #[cfg(debug_assertions)]
        {
            let mut current = self.head;
            while let Some(current_waiter) = current {
                assert_ne!(current_waiter, waiter, "waiter is already linked");
                // SAFETY: linked waiters are pinned in blocked lock() stack
                // frames, and this traversal holds the bucket lock.
                current = *unsafe { current_waiter.as_ref() }.next_mut();
            }

            // SAFETY: the pinned waiter remains in the blocked task's lock()
            // stack frame until unlock() removes it and wakes that task.
            debug_assert!(unsafe { waiter.as_ref() }.next_mut().is_none());
        }

        if let Some(tail) = self.tail {
            // SAFETY: every linked waiter is pinned in a blocked lock() stack
            // frame and cannot return until it has been removed from the list.
            *unsafe { tail.as_ref() }.next_mut() = Some(waiter);
        } else {
            self.head = Some(waiter);
        }
        self.tail = Some(waiter);
    }

    fn pop_front_for(&mut self, lock_key: usize) -> Option<Arc<dyn Task>> {
        let mut previous: Option<NonNull<WaiterOnStack>> = None;
        let mut current = self.head;

        while let Some(waiter_ptr) = current {
            // SAFETY: linked waiters are pinned in blocked lock() stack frames;
            // this bucket lock serializes traversal and removal.
            let waiter = unsafe { waiter_ptr.as_ref() };
            let waiter_next = waiter.next_mut();

            if waiter.lock_key == lock_key {
                let next = waiter_next.take();
                if let Some(previous) = previous {
                    // SAFETY: `previous` is a pinned waiter whose task remains
                    // blocked, and list mutation is serialized by the bucket.
                    let previous_next = unsafe { previous.as_ref() }.next_mut();
                    debug_assert_eq!(*previous_next, Some(waiter_ptr));
                    *previous_next = next;
                } else {
                    debug_assert_eq!(self.head, Some(waiter_ptr));
                    self.head = next;
                }

                if self.tail == Some(waiter_ptr) {
                    self.tail = previous;
                }
                debug_assert_eq!(self.head.is_none(), self.tail.is_none());
                return Some(waiter.task.clone());
            }

            previous = Some(waiter_ptr);
            current = *waiter_next;
        }

        None
    }
}

// SAFETY: raw waiter pointers are only accessed under the bucket lock. Each
// waiter is pinned in a blocked lock() stack frame and removed before wakeup.
unsafe impl Send for BucketSleepLockWaitList {}

struct Bucket {
    waiters: SpinMutex<BucketSleepLockWaitList>,
}

impl Bucket {
    const fn new() -> Self {
        Self {
            waiters: SpinMutex::new(BucketSleepLockWaitList::new()),
        }
    }
}

static WAIT_BUCKETS: [Bucket; WAIT_BUCKET_COUNT] = [const { Bucket::new() }; WAIT_BUCKET_COUNT];

fn wait_bucket(lock_key: usize) -> &'static Bucket {
    let hash = lock_key ^ (lock_key >> 7) ^ (lock_key >> 17);
    &WAIT_BUCKETS[hash % WAIT_BUCKET_COUNT]
}

pub struct BucketSleepLocker {
    #[cfg(feature = "no-smp")]
    locked: UnsafeCell<bool>,
    #[cfg(not(feature = "no-smp"))]
    locked: core::sync::atomic::AtomicBool,
}

impl BucketSleepLocker {
    pub const fn new() -> Self {
        Self {
            #[cfg(feature = "no-smp")]
            locked: UnsafeCell::new(false),
            #[cfg(not(feature = "no-smp"))]
            locked: core::sync::atomic::AtomicBool::new(false),
        }
    }

    fn key(&self) -> usize {
        self as *const Self as usize
    }

    fn set_unlocked(&self) {
        #[cfg(feature = "no-smp")]
        {
            // SAFETY: no-smp guarantees that only the current CPU can access
            // the lock state, while the bucket lock serializes waiter changes.
            unsafe { *self.locked.get() = false };
        }
        #[cfg(not(feature = "no-smp"))]
        self.locked.store(false, Ordering::Release);
    }
}

impl LockerTrait for BucketSleepLocker {
    fn try_lock(&self, _name: &'static str) -> bool {
        #[cfg(feature = "no-smp")]
        {
            // SAFETY: no-smp guarantees that lock state cannot be accessed by
            // another CPU concurrently.
            unsafe {
                if *self.locked.get() {
                    false
                } else {
                    *self.locked.get() = true;
                    true
                }
            }
        }
        #[cfg(not(feature = "no-smp"))]
        self.locked
            .compare_exchange(false, true, Ordering::Acquire, Ordering::Relaxed)
            .is_ok()
    }

    fn lock(&self, name: &'static str) {
        let lock_key = self.key();
        let bucket = wait_bucket(lock_key);
        let mut waiter = pin!(WaiterOnStack::new(lock_key, current::task().clone()));
        loop {
            if self.try_lock(name) {
                return;
            }

            {
                let mut waiters = bucket.waiters.lock();
                if self.try_lock(name) {
                    return;
                }
                // PERF_DEBUG(scheduler-time): Attribute bucket sleep-lock wait
                // time to the concrete lock name.
                scheduler::block_task_uninterruptible_named(
                    waiter.as_ref().get_ref().task.clone(),
                    "bucket_sleep_lock",
                    name,
                );
                waiters.push_back(waiter.as_mut());
            }

            current::schedule();
            let event = current::task().take_wakeup_event().unwrap();
            debug_assert_eq!(event, Event::SleepLock);
        }
    }

    fn unlock(&self, _name: &'static str) {
        let lock_key = self.key();
        let bucket = wait_bucket(lock_key);
        let task = {
            let mut waiters = bucket.waiters.lock();
            self.set_unlocked();
            waiters.pop_front_for(lock_key)
        };
        if let Some(task) = task {
            scheduler::wakeup_task_uninterruptible(task, Event::SleepLock);
        }
    }
}

// SAFETY: access to protected data is serialized by the locker state and the
// global bucket table serializes its waiter list.
unsafe impl Send for BucketSleepLocker {}

// SAFETY: access to protected data is serialized by the locker state and the
// global bucket table serializes its waiter list.
unsafe impl Sync for BucketSleepLocker {}

pub type BucketSleepLock<T> = Mutex<T, BucketSleepLocker>;
pub type BucketSleepLockGuard<'a, T> = LockGuard<'a, T, BucketSleepLocker>;

impl<T> BucketSleepLock<T> {
    pub const fn new(data: T, name: &'static str) -> Self {
        Self::create(data, name, BucketSleepLocker::new())
    }
}
