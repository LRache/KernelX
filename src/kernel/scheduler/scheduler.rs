use alloc::collections::vec_deque::VecDeque;
use alloc::sync::Arc;
use spin::Mutex;

use crate::kernel::scheduler::current;
use crate::kernel::task::{TaskState, TCB};
use crate::arch;

use super::processor::Processor;

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
    // kinfo!("Push task with TID {}", tcb.get_tid());
    SCHEDULER.push_task(tcb);
}

pub fn fetch_next_task() -> Option<Arc<TCB>> {
    SCHEDULER.fetch_next_task()
}

pub fn run_tasks(_hartid: u8) -> ! {
    current::clear();
    loop {
        arch::disable_interrupt();
        if let Some(mut tcb) = fetch_next_task() {
            tcb.run();

            let mut processor = Processor::new(&mut tcb);
            current::set(&mut processor);
            
            processor.switch_to_task();

            current::clear();

            tcb.with_state_mut(|state| {
                if state.state == TaskState::Running {
                    state.state = TaskState::Ready;
                    push_task(tcb.clone());
                } else {
                    // kinfo!("Task {} not ready to run, state: {:?}", tcb.get_tid(), state.state);
                }
            });
        } else {
            arch::enable_interrupt();
            arch::wait_for_interrupt();
        }
    }
}
