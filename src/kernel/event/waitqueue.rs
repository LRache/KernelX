use core::usize;
use alloc::sync::Arc;
use alloc::vec::Vec;
use alloc::collections::VecDeque;

use crate::kernel::scheduler::current;
use crate::kernel::task::TCB;

use super::Event;

struct WaitQueueItem<T: Copy> {
    tcb: Arc<TCB>,
    arg: T,
}

pub struct WaitQueue<T: Copy> {
    waiters: VecDeque<WaitQueueItem<T>>,
}

impl<T: Copy> WaitQueueItem<T> {
    fn new(tcb: Arc<TCB>, arg: T) -> Self {
        Self { tcb, arg }
    }

    fn wakeup(&self, e: Event) {
        self.tcb.wakeup_by_event(e);
    }
}

impl<T: Copy> WaitQueue<T> {
    pub fn new() -> Self {
        Self {
            waiters: VecDeque::new(),
        }
    }

    pub fn wait(&mut self, tcb: Arc<TCB>, arg: T) {
        self.waiters.push_back(WaitQueueItem::new(tcb, arg));
    }

    pub fn wait_current(&mut self, arg: T) {
        let current = current::tcb();
        current.block("waitqueue");
        self.wait(current.clone(), arg);
    }

    pub fn wake_one(&mut self, e: Event) -> Option<Arc<TCB>> {
        let r = self.waiters.pop_front();
        match r {
            Some(item) => {
                item.wakeup(e);
                Some(item.tcb)
            }
            None => None,
        }
    }

    pub fn wake_all(&mut self, map_arg_to_event: impl Fn(T) -> Event) -> Vec<Arc<TCB>> {
        self.waiters.iter().for_each(|i| i.wakeup(map_arg_to_event(i.arg)));
        self.waiters.drain(..).map(|item| item.tcb).collect()
    }
}
