use core::usize;
use alloc::sync::Arc;
use alloc::vec::Vec;
use alloc::collections::VecDeque;

use crate::kernel::scheduler::current;
use crate::kernel::task::TCB;

use super::Event;

struct WaitQueueItem {
    tcb: Arc<TCB>,
    waker: Option<usize>,
}

pub struct WaitQueue {
    waiters: VecDeque<WaitQueueItem>,
}

impl WaitQueueItem {
    fn new(tcb: Arc<TCB>, waker: Option<usize>) -> Self {
        Self { tcb, waker: waker }
    }

    fn wakeup(&self, e: Event) {
        match self.waker {
            None => self.tcb.wakeup(),
            Some(waker) => self.tcb.wakeup_by_event(waker, e),
        }
    }
}

impl WaitQueue {
    pub fn new() -> Self {
        Self {
            waiters: VecDeque::new(),
        }
    }

    pub fn wait(&mut self, tcb: Arc<TCB>, waker: Option<usize>) {
        self.waiters.push_back(WaitQueueItem::new(tcb, waker));
    }

    pub fn wait_current(&mut self, waker: Option<usize>) {
        let current = current::tcb();
        current.block();
        self.wait(current.clone(), waker);
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

    pub fn wake_all(&mut self, e: Event) -> Vec<Arc<TCB>> {
        self.waiters.iter().for_each(|t| t.wakeup(e));
        self.waiters.drain(..).map(|item| item.tcb).collect()
    }
}
