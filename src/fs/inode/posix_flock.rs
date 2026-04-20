use alloc::collections::LinkedList;
use alloc::vec::Vec;

use crate::kernel::event::{Event, WaitQueue};
use crate::kernel::scheduler::Tid;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PosixFlockType {
    Read,
    Write,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PosixFlock {
    pub owner: Tid,
    pub lock_type: PosixFlockType,
    pub start: i64,
    pub len: i64,
}

impl PosixFlock {
    pub const fn new(owner: Tid, lock_type: PosixFlockType, start: i64, len: i64) -> Self {
        Self {
            owner,
            lock_type,
            start,
            len,
        }
    }
}

pub struct PosixFlockState {
    locks: LinkedList<PosixFlock>,
    waiters: WaitQueue<()>,
}

impl PosixFlockState {
    pub fn new() -> Self {
        Self {
            locks: LinkedList::new(),
            waiters: WaitQueue::new(),
        }
    }

    pub fn get_conflict(
        &self,
        owner: Tid,
        request_type: PosixFlockType,
        request_start: i64,
        request_len: i64,
    ) -> Option<PosixFlock> {
        let request_range = LockRange {
            start: request_start,
            end: if request_len == 0 {
                None
            } else {
                Some(request_start + request_len)
            },
        };

        self.locks
            .iter()
            .copied()
            .find(|lock| lock.owner != owner && ranges_conflict(*lock, request_type, request_range))
    }

    pub fn apply(&mut self, owner: Tid, request_type: Option<PosixFlockType>, request_start: i64, request_len: i64) {
        let request_range = LockRange {
            start: request_start,
            end: if request_len == 0 {
                None
            } else {
                Some(request_start + request_len)
            },
        };

        let mut rebuilt = Vec::new();
        for lock in self.locks.iter().copied() {
            if lock.owner != owner {
                rebuilt.push(lock);
            } else {
                subtract_range(lock, request_range, &mut rebuilt);
            }
        }

        if let Some(lock_type) = request_type {
            rebuilt.push(PosixFlock::new(owner, lock_type, request_start, request_len));
        }

        self.locks = merge_same_owner_locks(rebuilt).into_iter().collect();
    }

    pub fn remove_owner(&mut self, owner: Tid) -> bool {
        let old_len = self.locks.len();
        self.locks.retain(|lock| lock.owner != owner);
        self.locks.len() != old_len
    }

    pub fn wait_current(&mut self) {
        self.waiters.wait_current(());
    }

    pub fn wake_all(&mut self) {
        self.waiters.wake_all(|_| Event::IOComplete);
    }
}

impl Default for PosixFlockState {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Clone, Copy)]
struct LockRange {
    start: i64,
    end: Option<i64>,
}

impl LockRange {
    fn from_lock(lock: PosixFlock) -> Self {
        let end = if lock.len == 0 {
            None
        } else {
            Some(
                lock.start
                    .checked_add(lock.len)
                    .expect("PosixFlock::len must be normalized before use"),
            )
        };
        Self { start: lock.start, end }
    }

    fn to_len(self) -> i64 {
        self.end.map_or(0, |end| end - self.start)
    }

    fn overlaps(self, other: Self) -> bool {
        self.end.is_none_or(|end| other.start < end) && other.end.is_none_or(|end| self.start < end)
    }

    fn touches_or_overlaps(self, other: Self) -> bool {
        self.end.is_none_or(|end| other.start <= end) && other.end.is_none_or(|end| self.start <= end)
    }
}

fn ranges_conflict(existing: PosixFlock, request_type: PosixFlockType, request_range: LockRange) -> bool {
    LockRange::from_lock(existing).overlaps(request_range)
        && (existing.lock_type == PosixFlockType::Write || request_type == PosixFlockType::Write)
}

fn push_fragment(fragments: &mut Vec<PosixFlock>, owner: Tid, lock_type: PosixFlockType, range: LockRange) {
    if matches!(range.end, Some(end) if end <= range.start) {
        return;
    }

    fragments.push(PosixFlock::new(owner, lock_type, range.start, range.to_len()));
}

fn subtract_range(lock: PosixFlock, removed: LockRange, fragments: &mut Vec<PosixFlock>) {
    let current = LockRange::from_lock(lock);
    if !current.overlaps(removed) {
        fragments.push(lock);
        return;
    }

    if current.start < removed.start {
        push_fragment(
            fragments,
            lock.owner,
            lock.lock_type,
            LockRange {
                start: current.start,
                end: Some(removed.start),
            },
        );
    }

    if let Some(removed_end) = removed.end
        && current.end.is_none_or(|end| removed_end < end)
    {
        push_fragment(
            fragments,
            lock.owner,
            lock.lock_type,
            LockRange {
                start: removed_end,
                end: current.end,
            },
        );
    }
}

fn merge_same_owner_locks(mut locks: Vec<PosixFlock>) -> Vec<PosixFlock> {
    locks.sort_by(|lhs, rhs| {
        lhs.owner
            .cmp(&rhs.owner)
            .then(lhs.start.cmp(&rhs.start))
            .then(lhs.len.cmp(&rhs.len))
    });

    let mut merged = Vec::new();
    for lock in locks {
        let can_merge = merged.last().is_some_and(|last: &PosixFlock| {
            last.owner == lock.owner
                && last.lock_type == lock.lock_type
                && LockRange::from_lock(*last).touches_or_overlaps(LockRange::from_lock(lock))
        });

        if can_merge {
            let last = merged.last_mut().unwrap();
            let end = match (LockRange::from_lock(*last).end, LockRange::from_lock(lock).end) {
                (None, _) | (_, None) => None,
                (Some(lhs), Some(rhs)) => Some(lhs.max(rhs)),
            };
            last.len = end.map_or(0, |value| value - last.start);
        } else {
            merged.push(lock);
        }
    }

    merged
}
