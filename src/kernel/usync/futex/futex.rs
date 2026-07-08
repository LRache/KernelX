use alloc::collections::{BTreeMap, LinkedList};
use alloc::sync::Arc;
use alloc::vec::Vec;

use crate::kernel::errno::{Errno, SysResult};
use crate::kernel::event::Event;
use crate::kernel::mm::{AddrSpace, PhysPageFrame};
use crate::kernel::scheduler;
use crate::kernel::scheduler::{Task, current};
use crate::klib::SpinLock;

struct FutexWaitQueueItem {
    tcb: Arc<dyn Task>,
    bitset: u32,
    kind: FutexWaitKind,
}

#[derive(Clone, Copy)]
enum FutexWaitKind {
    Wait,
    Waitv { index: usize },
}

fn wake_waiter(item: FutexWaitQueueItem) -> bool {
    let event = match item.kind {
        FutexWaitKind::Wait => Event::Futex,
        FutexWaitKind::Waitv { index } => Event::FutexWaitv { index },
    };

    scheduler::wakeup_task(item.tcb, event).is_ok()
}

pub struct Futex {
    wait_list: LinkedList<FutexWaitQueueItem>,
}

impl Futex {
    pub fn new() -> Self {
        Self {
            wait_list: LinkedList::new(),
        }
    }

    fn wait_current(&mut self, bitset: u32, kind: FutexWaitKind) {
        self.wait_list.push_back(FutexWaitQueueItem {
            tcb: current::task().clone(),
            bitset,
            kind,
        });
    }

    pub fn wake(&mut self, num: usize, mask: u32) -> SysResult<usize> {
        if num == 0 {
            return Ok(0);
        }

        let mut woken = 0;
        let mut cursor = self.wait_list.cursor_front_mut();
        while let Some(item) = cursor.current() {
            if (item.bitset & mask) != 0 {
                let item = cursor.remove_current().unwrap();

                if wake_waiter(item) {
                    woken += 1;
                    if woken >= num {
                        break;
                    }
                }
            } else {
                cursor.move_next();
            }
        }

        Ok(woken)
    }

    pub fn remove_waiter(&mut self, task: &Arc<dyn Task>) -> usize {
        let mut removed = 0;
        let mut cursor = self.wait_list.cursor_front_mut();
        while let Some(item) = cursor.current() {
            if Arc::ptr_eq(&item.tcb, task) {
                cursor.remove_current();
                removed += 1;
            } else {
                cursor.move_next();
            }
        }
        removed
    }

    fn is_empty(&self) -> bool {
        self.wait_list.is_empty()
    }
}

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum FutexKey {
    Private { addrspace: usize, uaddr: usize },
    Shared { page: usize, offset: usize },
}

impl FutexKey {
    pub fn private(addrspace: &Arc<AddrSpace>, uaddr: usize) -> Self {
        Self::Private {
            addrspace: Arc::as_ptr(addrspace) as usize,
            uaddr,
        }
    }

    pub fn shared(frame: &Arc<PhysPageFrame>, offset: usize) -> Self {
        Self::Shared {
            page: Arc::as_ptr(frame) as usize,
            offset,
        }
    }
}

#[derive(Clone, Copy)]
pub struct FutexAddr {
    key: FutexKey,
    value: i32,
}

impl FutexAddr {
    pub fn new(key: FutexKey, value: i32) -> Self {
        Self { key, value }
    }

    pub fn key(&self) -> FutexKey {
        self.key
    }

    fn value(&self) -> i32 {
        self.value
    }
}

static FUTEXES: SpinLock<BTreeMap<FutexKey, SpinLock<Futex>>> = SpinLock::new(BTreeMap::new(), "static::FUTEXES");

fn check_current_value(addr: FutexAddr, expected: i32) -> SysResult<()> {
    if addr.value() != expected {
        return Err(Errno::EAGAIN);
    }

    Ok(())
}

pub fn wait_current(addr: FutexAddr, expected: i32, bitset: u32) -> SysResult<()> {
    wait_current_kind(addr, expected, bitset, FutexWaitKind::Wait)
}

pub fn wait_current_waitv(addr: FutexAddr, expected: i32, bitset: u32, index: usize) -> SysResult<()> {
    wait_current_kind(addr, expected, bitset, FutexWaitKind::Waitv { index })
}

fn wait_current_kind(addr: FutexAddr, expected: i32, bitset: u32, kind: FutexWaitKind) -> SysResult<()> {
    let mut futexes = FUTEXES.lock();
    check_current_value(addr, expected)?;
    let futex = futexes
        .entry(addr.key())
        .or_insert_with(|| SpinLock::new(Futex::new(), "Futex"));

    let mut futex = futex.lock();
    futex.wait_current(bitset, kind);
    Ok(())
}

pub fn wake(key: FutexKey, num: usize, mask: u32) -> SysResult<usize> {
    let mut futexes = FUTEXES.lock();
    let (woken, empty) = {
        let Some(futex) = futexes.get(&key) else {
            return Ok(0);
        };
        let mut futex = futex.lock();
        let woken = futex.wake(num, mask)?;
        (woken, futex.is_empty())
    };

    if empty {
        futexes.remove(&key);
    }

    Ok(woken)
}

pub fn requeue(
    addr: FutexAddr,
    key2: FutexKey,
    wake_count: usize,
    requeue_count: usize,
    val: Option<i32>,
) -> SysResult<usize> {
    let mut futexes = FUTEXES.lock();
    let mut pending = LinkedList::new();

    if let Some(val) = val {
        check_current_value(addr, val)?;
    }

    let key = addr.key();
    let (woken, moved, empty) = if let Some(futex_spinlock) = futexes.get(&key) {
        let mut futex = futex_spinlock.lock();
        let woken = futex.wake(wake_count, u32::MAX)?;
        let mut moved = 0;
        let mut cursor = futex.wait_list.cursor_front_mut();
        while moved < requeue_count {
            let Some(item) = cursor.remove_current() else {
                break;
            };
            pending.push_back(item);
            moved += 1;
        }
        (woken, moved, futex.is_empty())
    } else {
        return Ok(0);
    };

    if moved == 0 {
        if empty {
            futexes.remove(&key);
        }
        return Ok(woken);
    }

    if empty && key != key2 {
        futexes.remove(&key);
    }

    let futex2_spinlock = futexes
        .entry(key2)
        .or_insert_with(|| SpinLock::new(Futex::new(), "Futex"));
    let mut futex2 = futex2_spinlock.lock();
    futex2.wait_list.append(&mut pending);

    Ok(woken + moved)
}

pub fn cancel_wait_all(task: &Arc<dyn Task>) -> usize {
    let mut futexes = FUTEXES.lock();
    let mut removed = 0;
    let mut empty_keys = Vec::new();

    for (key, futex) in futexes.iter() {
        let (removed_count, empty) = {
            let mut futex = futex.lock();
            let removed_count = futex.remove_waiter(task);
            (removed_count, futex.is_empty())
        };

        removed += removed_count;
        if empty {
            empty_keys.push(*key);
        }
    }

    for key in empty_keys {
        futexes.remove(&key);
    }

    removed
}
