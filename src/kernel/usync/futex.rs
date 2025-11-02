use alloc::collections::BTreeMap;
use alloc::collections::LinkedList;
use alloc::sync::Arc;

use crate::kernel::errno::{Errno, SysResult};
use crate::kernel::event::Event;
use crate::kernel::task::TCB;
use crate::kernel::scheduler::current;
use crate::klib::SpinLock;

struct FutexWaitQueueItem {
    tcb: Arc<TCB>,
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
            tcb: current::tcb().clone(),
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
                {
                    let mut state = item.tcb.state().lock();
                    state.event = None;
                }
                
                
                item.tcb.wakeup(Event::Futex);
                
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

pub struct FutexManager {
    futexes: SpinLock<BTreeMap<usize, SpinLock<Futex>>>,
}

impl FutexManager {
    const fn new() -> Self {
        Self {
            futexes: SpinLock::new(BTreeMap::new()),
        }
    }

    fn wait_current(&self, kaddr: usize, expected: i32, mask: u32) -> SysResult<()> {
        let mut futexes = self.futexes.lock();
        let futex = futexes.entry(kaddr).or_insert_with(|| SpinLock::new(Futex::new(unsafe { &*(kaddr as *const i32) })));
        
        let mut futex = futex.lock();
        futex.wait_current(expected, mask)
    }

    fn wake(&self, kaddr: usize, num: usize, mask: u32) -> SysResult<usize> {
        let futexes = self.futexes.lock();
        if let Some(futex) = futexes.get(&kaddr) {
            let mut futex = futex.lock();
            futex.wake(num, mask)
        } else {
            Ok(0)
        }
    }
}

static FUTEX_MANAGER: FutexManager = FutexManager::new();

pub fn wait_current(kaddr: usize, expected: i32, bitset: u32) -> SysResult<()> {
    FUTEX_MANAGER.wait_current(kaddr, expected, bitset)
}

pub fn wake(kaddr: usize, num: usize, mask: u32) -> SysResult<usize> {
    FUTEX_MANAGER.wake(kaddr, num, mask)
}
