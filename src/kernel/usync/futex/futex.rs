use alloc::collections::{BTreeMap, LinkedList};
use alloc::sync::Arc;
use alloc::vec::Vec;

use crate::kernel::errno::{Errno, SysResult};
use crate::kernel::event::Event;
use crate::kernel::mm::AddrSpace;
use crate::kernel::scheduler;
use crate::kernel::scheduler::{Task, current};
use crate::klib::SleepLock;

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

struct Futex {
    wait_list: LinkedList<FutexWaitQueueItem>,
}

impl Futex {
    fn new() -> Self {
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

    fn wake(&mut self, num: usize, mask: u32) -> SysResult<usize> {
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

    fn remove_waiter(&mut self, task: &Arc<dyn Task>) -> usize {
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

    pub fn shared(frame: &crate::kernel::mm::maparea::PinPageFrame, offset: usize) -> Self {
        Self::Shared {
            page: frame.kpage(),
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

pub struct FutexManager {
    futexes: BTreeMap<FutexKey, Futex>,
}

impl FutexManager {
    const fn new() -> Self {
        Self {
            futexes: BTreeMap::new(),
        }
    }

    pub fn wait_current(&mut self, addr: FutexAddr, expected: i32, bitset: u32) -> SysResult<()> {
        self.wait_current_kind(addr, expected, bitset, FutexWaitKind::Wait)
    }

    pub fn touch(&mut self, addr: FutexAddr, expected: i32) -> SysResult<()> {
        check_current_value(addr, expected)
    }

    fn wait_current_kind(&mut self, addr: FutexAddr, expected: i32, bitset: u32, kind: FutexWaitKind) -> SysResult<()> {
        check_current_value(addr, expected)?;
        current::task().block(match kind {
            FutexWaitKind::Wait => "futex",
            FutexWaitKind::Waitv { .. } => "futex_waitv",
        });
        self.wait_current_unchecked(addr, bitset, kind);
        Ok(())
    }

    pub fn wait_current_waitv_unchecked(&mut self, addr: FutexAddr, bitset: u32, index: usize) {
        self.wait_current_unchecked(addr, bitset, FutexWaitKind::Waitv { index });
    }

    fn wait_current_unchecked(&mut self, addr: FutexAddr, bitset: u32, kind: FutexWaitKind) {
        let futex = self.futexes.entry(addr.key()).or_insert_with(Futex::new);
        futex.wait_current(bitset, kind);
    }

    pub fn wake(&mut self, key: FutexKey, num: usize, mask: u32) -> SysResult<usize> {
        let (woken, empty) = {
            let Some(futex) = self.futexes.get_mut(&key) else {
                return Ok(0);
            };
            let woken = futex.wake(num, mask)?;
            (woken, futex.is_empty())
        };

        if empty {
            self.futexes.remove(&key);
        }

        Ok(woken)
    }

    pub fn requeue(
        &mut self,
        addr: FutexAddr,
        key2: FutexKey,
        wake_count: usize,
        requeue_count: usize,
        val: Option<i32>,
    ) -> SysResult<usize> {
        let mut pending = LinkedList::new();

        if let Some(val) = val {
            check_current_value(addr, val)?;
        }

        let key = addr.key();
        let (woken, moved, empty) = if let Some(futex) = self.futexes.get_mut(&key) {
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
                self.futexes.remove(&key);
            }
            return Ok(woken);
        }

        if empty && key != key2 {
            self.futexes.remove(&key);
        }

        let futex2 = self.futexes.entry(key2).or_insert_with(Futex::new);
        futex2.wait_list.append(&mut pending);

        Ok(woken + moved)
    }

    pub fn cancel_wait_all(&mut self, task: &Arc<dyn Task>) -> usize {
        let mut removed = 0;
        let mut empty_keys = Vec::new();

        for (key, futex) in self.futexes.iter_mut() {
            let removed_count = futex.remove_waiter(task);

            removed += removed_count;
            if futex.is_empty() {
                empty_keys.push(*key);
            }
        }

        for key in empty_keys {
            self.futexes.remove(&key);
        }

        removed
    }
}

static FUTEX_MANAGER: SleepLock<FutexManager> = SleepLock::new(FutexManager::new(), "static::FUTEX_MANAGER");

pub fn manager() -> impl core::ops::DerefMut<Target = FutexManager> {
    FUTEX_MANAGER.lock()
}

fn check_current_value(addr: FutexAddr, expected: i32) -> SysResult<()> {
    if addr.value() != expected {
        return Err(Errno::EAGAIN);
    }

    Ok(())
}
