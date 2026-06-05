use core::cell::UnsafeCell;
use core::ops::{Deref, DerefMut};

#[cfg(not(feature = "no-smp"))]
use core::hint::spin_loop;

const WRITE_LOCKED: usize = 1 << (usize::BITS as usize - 1);
const READER_MASK: usize = !WRITE_LOCKED;

pub struct ReadGuard<'a, T> {
    lock: &'a RWLock<T>,
}

impl<T> Deref for ReadGuard<'_, T> {
    type Target = T;

    fn deref(&self) -> &Self::Target {
        unsafe { &*self.lock.data.get() }
    }
}

impl<T> Drop for ReadGuard<'_, T> {
    fn drop(&mut self) {
        self.lock.read_unlock();
    }
}

pub struct WriteGuard<'a, T> {
    lock: &'a RWLock<T>,
}

impl<T> Deref for WriteGuard<'_, T> {
    type Target = T;

    fn deref(&self) -> &Self::Target {
        unsafe { &*self.lock.data.get() }
    }
}

impl<T> DerefMut for WriteGuard<'_, T> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        unsafe { &mut *self.lock.data.get() }
    }
}

impl<T> Drop for WriteGuard<'_, T> {
    fn drop(&mut self) {
        self.lock.write_unlock();
    }
}

pub struct RWLock<T> {
    state: State,
    data: UnsafeCell<T>,
}

impl<T> RWLock<T> {
    pub const fn new(data: T, _name: &'static str) -> Self {
        Self {
            state: State::new(),
            data: UnsafeCell::new(data),
        }
    }

    pub fn read(&self) -> ReadGuard<'_, T> {
        self.state.read_lock();
        ReadGuard { lock: self }
    }

    pub fn write(&self) -> WriteGuard<'_, T> {
        self.state.write_lock();
        WriteGuard { lock: self }
    }

    fn read_unlock(&self) {
        self.state.read_unlock();
    }

    fn write_unlock(&self) {
        self.state.write_unlock();
    }
}

unsafe impl<T: Send> Send for RWLock<T> {}
unsafe impl<T: Send + Sync> Sync for RWLock<T> {}

struct State {
    #[cfg(feature = "no-smp")]
    value: UnsafeCell<usize>,

    #[cfg(not(feature = "no-smp"))]
    value: core::sync::atomic::AtomicUsize,
}

impl State {
    const fn new() -> Self {
        Self {
            #[cfg(feature = "no-smp")]
            value: UnsafeCell::new(0),
            #[cfg(not(feature = "no-smp"))]
            value: core::sync::atomic::AtomicUsize::new(0),
        }
    }

    fn read_lock(&self) {
        #[cfg(feature = "no-smp")]
        unsafe {
            let state = &mut *self.value.get();
            if *state & WRITE_LOCKED != 0 {
                panic!("Deadlock detected in RWLock: cannot acquire read lock while write-locked on single-core");
            }
            let readers = *state & READER_MASK;
            debug_assert!(readers < READER_MASK);
            *state += 1;
            return;
        }

        #[cfg(not(feature = "no-smp"))]
        while !self.try_read_lock() {
            spin_loop();
        }
    }

    fn write_lock(&self) {
        #[cfg(feature = "no-smp")]
        unsafe {
            let state = &mut *self.value.get();
            if *state != 0 {
                panic!("Deadlock detected in RWLock: cannot acquire write lock while already locked on single-core");
            }
            *state = WRITE_LOCKED;
            return;
        }

        #[cfg(not(feature = "no-smp"))]
        while !self.try_write_lock() {
            spin_loop();
        }
    }

    fn read_unlock(&self) {
        #[cfg(feature = "no-smp")]
        unsafe {
            let state = &mut *self.value.get();
            debug_assert!(*state & WRITE_LOCKED == 0);
            debug_assert!(*state & READER_MASK != 0);
            *state -= 1;
            return;
        }

        #[cfg(not(feature = "no-smp"))]
        {
            let prev = self.value.fetch_sub(1, core::sync::atomic::Ordering::Release);
            debug_assert!(prev & WRITE_LOCKED == 0);
            debug_assert!(prev & READER_MASK != 0);
        }
    }

    fn write_unlock(&self) {
        #[cfg(feature = "no-smp")]
        unsafe {
            let state = &mut *self.value.get();
            debug_assert!(*state == WRITE_LOCKED);
            *state = 0;
            return;
        }

        #[cfg(not(feature = "no-smp"))]
        {
            debug_assert!(self.value.load(core::sync::atomic::Ordering::Relaxed) == WRITE_LOCKED);
            self.value.store(0, core::sync::atomic::Ordering::Release);
        }
    }

    #[cfg(not(feature = "no-smp"))]
    fn try_read_lock(&self) -> bool {
        loop {
            let state = self.value.load(core::sync::atomic::Ordering::Relaxed);
            if state & WRITE_LOCKED != 0 {
                return false;
            }

            let readers = state & READER_MASK;
            debug_assert!(readers < READER_MASK);
            if readers == READER_MASK {
                return false;
            }

            if self
                .value
                .compare_exchange_weak(
                    state,
                    state + 1,
                    core::sync::atomic::Ordering::Acquire,
                    core::sync::atomic::Ordering::Relaxed,
                )
                .is_ok()
            {
                return true;
            }
        }
    }

    #[cfg(not(feature = "no-smp"))]
    fn try_write_lock(&self) -> bool {
        self.value
            .compare_exchange(
                0,
                WRITE_LOCKED,
                core::sync::atomic::Ordering::Acquire,
                core::sync::atomic::Ordering::Relaxed,
            )
            .is_ok()
    }
}

#[cfg(feature = "no-smp")]
unsafe impl Send for State {}
#[cfg(feature = "no-smp")]
unsafe impl Sync for State {}
