use alloc::vec::Vec;

use crate::kernel::event::{Event, WaitQueue};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BsdFlockType {
    Shared,
    Exclusive,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct BsdFlock {
    pub owner: usize,
    pub lock_type: BsdFlockType,
}

impl BsdFlock {
    pub const fn new(owner: usize, lock_type: BsdFlockType) -> Self {
        Self { owner, lock_type }
    }
}

pub(crate) struct BsdFlockState {
    locks: Vec<BsdFlock>,
    waiters: WaitQueue<()>,
}

impl BsdFlockState {
    pub fn new() -> Self {
        Self {
            locks: Vec::new(),
            waiters: WaitQueue::new(),
        }
    }

    pub fn get_conflict(&self, owner: usize, request_type: BsdFlockType) -> Option<BsdFlock> {
        self.locks.iter().copied().find(|lock| {
            lock.owner != owner
                && match request_type {
                    BsdFlockType::Shared => lock.lock_type == BsdFlockType::Exclusive,
                    BsdFlockType::Exclusive => true,
                }
        })
    }

    pub fn apply(&mut self, owner: usize, request_type: Option<BsdFlockType>) {
        self.locks.retain(|lock| lock.owner != owner);
        if let Some(lock_type) = request_type {
            self.locks.push(BsdFlock::new(owner, lock_type));
        }
    }

    pub fn remove_owner(&mut self, owner: usize) -> bool {
        let old_len = self.locks.len();
        self.locks.retain(|lock| lock.owner != owner);
        self.locks.len() != old_len
    }

    pub fn wait_current(&mut self) {
        self.waiters.wait_current(());
    }

    pub fn remove_current_waiter(&mut self) {
        self.waiters.remove_current();
    }

    pub fn wake_all(&mut self) {
        self.waiters.wake_all(|_| Event::IOComplete);
    }
}

impl Default for BsdFlockState {
    fn default() -> Self {
        Self::new()
    }
}
