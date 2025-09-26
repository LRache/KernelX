use crate::kernel::scheduler::current;
use crate::kernel::task::{ThreadState, TCB};
use crate::{arch, kdebug};
use super::processor::Processor;

use alloc::collections::vec_deque::VecDeque;
use alloc::sync::Arc;
use spin::Mutex;

pub struct Scheduler {
    ready_queue: Mutex<VecDeque<Arc<TCB>>>,
}

impl Scheduler {
    pub const fn new() -> Self {
        Self {
            ready_queue: Mutex::new(VecDeque::new()),
        }
    }

    pub fn push_task(&self, tcb: Arc<TCB>) {
        self.ready_queue.lock().push_back(tcb);
    }

    pub fn fetch_next_task(&self) -> Option<Arc<TCB>> {
        self.ready_queue.lock().pop_front()
    }
}

static SCHEDULER: Scheduler = Scheduler::new();

pub fn push_task(tcb: Arc<TCB>) {
    SCHEDULER.push_task(tcb);
}

pub fn fetch_next_task() -> Option<Arc<TCB>> {
    SCHEDULER.fetch_next_task()
}

pub fn run_tasks() -> ! {
    current::clear();
    loop {
        if let Some(mut tcb) = fetch_next_task() {
            assert!(tcb.get_state() == ThreadState::Ready);
            
            tcb.set_state(ThreadState::Running);
            
            let mut processor = Processor::new(&mut tcb);
            current::set(&mut processor);
            
            processor.switch_to_task();

            current::clear();

            if tcb.get_state() == ThreadState::Running {
                tcb.set_state(ThreadState::Ready);
                push_task(tcb);
            }
        } else {
            arch::enable_interrupt();
            arch::wait_for_interrupt();
        }
    }
}
