use alloc::sync::Arc;
use alloc::vec::Vec;
use alloc::collections::VecDeque;
use spin::Mutex;

use crate::kernel::task::TCB;

pub struct WaitQueue {
    waiters: Mutex<VecDeque<Arc<TCB>>>,
}

impl WaitQueue {
    pub fn new() -> Self {
        Self {
            waiters: Mutex::new(VecDeque::new()),
        }
    }

    pub fn wait(&self, tcb: Arc<TCB>) {
        let mut waiters = self.waiters.lock();
        waiters.push_back(tcb);
    }

    pub fn wake_one(&self) -> Option<Arc<TCB>> {
        let mut waiters = self.waiters.lock();
        let tcb = waiters.pop_front();
        match &tcb {
            Some(t) => {
                t.wakeup();
            },
            None => {},
        }
        tcb
    }

    pub fn wake_all(&self) -> Vec<Arc<TCB>> {
        let mut waiters = self.waiters.lock();
        waiters.iter().for_each(|t| t.wakeup());
        waiters.drain(..).collect()
    }
}
