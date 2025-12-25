use alloc::collections::BTreeMap;
use alloc::collections::LinkedList;
use alloc::sync::Arc;

use crate::kernel::errno::{Errno, SysResult};
use crate::kernel::event::Event;
use crate::kernel::scheduler;
use crate::kernel::scheduler::Task;
use crate::kernel::scheduler::current;
use crate::klib::SpinLock;

struct FutexWaitQueueItem {
    tcb: Arc<dyn Task>,
    bitset: u32,
}

pub struct Futex {
    kvalue: &'static i32,
    wait_list: LinkedList<FutexWaitQueueItem>,
}

impl Futex {
    pub fn new(kvalue: &'static i32) -> Self {
        Self {
            kvalue,
            wait_list: LinkedList::new(),
        }
    }

    pub fn wait_current(&mut self, expected: i32, bitset: u32) -> SysResult<()> {
        if *self.kvalue != expected {
            return Err(Errno::EAGAIN);
        }
        
        self.wait_list.push_back(FutexWaitQueueItem {
            tcb: current::task().clone(),
            bitset,
        });
        
        Ok(())
    }

    pub fn wake(&mut self, num: usize, mask: u32) -> SysResult<usize> {
        let mut woken = 0;
        let mut cursor = self.wait_list.cursor_front_mut();
        while let Some(item) = cursor.current() {
            if (item.bitset & mask) != 0 {
                let item = cursor.remove_current().unwrap();
                
                scheduler::wakeup_task(item.tcb.clone(), Event::Futex);
                
                woken += 1;
                if woken >= num {
                    break;
                }
            } else {
                cursor.move_next();
            }
        }

        Ok(woken)
    }
}

static FUTEXES: SpinLock<BTreeMap<usize, SpinLock<Futex>>> = SpinLock::new(BTreeMap::new());

pub fn wait_current(kaddr: usize, expected: i32, bitset: u32) -> SysResult<()> {
    let mut futexes = FUTEXES.lock();
    let futex = futexes.entry(kaddr).or_insert_with(|| SpinLock::new(Futex::new(unsafe { &*(kaddr as *const i32) })));
        
    let mut futex = futex.lock();
    futex.wait_current(expected, bitset)
}


pub fn wake(kaddr: usize, num: usize, mask: u32) -> SysResult<usize> {
    let futexes = FUTEXES.lock();
    if let Some(futex) = futexes.get(&kaddr) {
        let mut futex = futex.lock();
        futex.wake(num, mask)
    } else {
        Ok(0)
    }
}

pub fn requeue(kaddr: usize, kaddr2: usize, num: usize, val: Option<i32>) -> SysResult<usize> {
    let mut futexes = FUTEXES.lock();
    let mut pending = LinkedList::new();

    let moved = if let Some(futex_spinlock) = futexes.get(&kaddr) {
        let mut futex = futex_spinlock.lock();
        if let Some(val) = val {
            if *futex.kvalue != val {
                return Err(Errno::EAGAIN);
            }
        }

        let mut moved = 0;
        let mut cursor = futex.wait_list.cursor_front_mut();
        while let Some(item) = cursor.remove_current() {
            pending.push_back(item);
            moved += 1;
            if moved >= num {
                break;
            }
        }
        moved
    } else {
        return Ok(0);
    };

    let futex2_spinlock = futexes.entry(kaddr2).or_insert_with(|| SpinLock::new(Futex::new(unsafe { &*(kaddr2 as *const i32) })));
    let mut futex2 = futex2_spinlock.lock();
    futex2.wait_list.append(&mut pending);

    Ok(moved)
}
