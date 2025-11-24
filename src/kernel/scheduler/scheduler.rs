use alloc::collections::vec_deque::VecDeque;
use alloc::sync::Arc;

use crate::kernel::scheduler::current;
use crate::kernel::scheduler::task::Task;
use crate::kernel::event::Event;
use crate::arch;
use crate::klib::SpinLock;

use super::processor::Processor;

pub struct Scheduler {
    ready_queue: SpinLock<VecDeque<Arc<dyn Task>>>,
}

impl Scheduler {
    const fn new() -> Self {
        Self {
            ready_queue: SpinLock::new(VecDeque::new()),
        }
    }

    fn push_task(&self, task: Arc<dyn Task>) {
        let mut ready_queue = self.ready_queue.lock();
        ready_queue.iter().for_each(|t| {
            debug_assert!(t.tid() != task.tid(), "Task {} is already in ready queue!", t.tid());
        });
        ready_queue.push_back(task);
    }

    fn fetch_next_task(&self) -> Option<Arc<dyn Task>> {
        self.ready_queue.lock().pop_front()
    }
}

static SCHEDULER: Scheduler = Scheduler::new();

pub fn push_task(task: Arc<dyn Task>) {
    SCHEDULER.push_task(task);
}

pub fn fetch_next_task() -> Option<Arc<dyn Task>> {
    SCHEDULER.fetch_next_task()
}

pub fn block_task(task: &Arc<dyn Task>, reason: &str) {
    task.block(reason);
}

pub fn block_task_uninterruptible(task: &Arc<dyn Task>, reason: &str) {
    task.block_uninterruptible(reason);
}

pub fn wakeup_task(task: Arc<dyn Task>, event: Event) {
    if task.wakeup(event) {
        push_task(task);
    }
}

pub fn run_tasks(_hartid: u8) -> ! {
    current::clear();
    loop {
        arch::disable_interrupt();
        if let Some(task) = fetch_next_task() {
            // crate::kinfo!("Switching to task {}", task.tid());
            if !task.run_if_ready() {
                continue;
            }

            let mut processor = Processor::new(&task);
            
            processor.switch_to_task();

            if task.state_running_to_ready() {
                // kinfo!("Task {} yielded CPU, push back to ready queue", task.tid());
                push_task(task);
            } else {
                // kinfo!("Task {} switched out of CPU", task.tid());
            }
        } else {
            arch::enable_interrupt();
            arch::wait_for_interrupt();
        }
    }
}
