use alloc::collections::vec_deque::VecDeque;
use alloc::sync::Arc;

use crate::kernel::scheduler::current;
use crate::kernel::scheduler::task::Task;
use crate::kernel::event::Event;
use crate::klib::SpinLock;
use crate::arch;

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

pub fn wakeup_task(task: Arc<dyn Task>, event: Event) {
    if task.wakeup(event) {
        push_task(task);
    }
}

pub fn wakeup_task_uninterruptible(task: Arc<dyn Task>, event: Event) {
    task.wakeup_uninterruptible(event);
    push_task(task);
}

pub fn run_tasks(hartid: usize) -> ! {
    let mut processor = Processor::new(hartid);
    current::set(&processor);
    loop {
        arch::disable_interrupt();
        if let Some(task) = fetch_next_task() {
            if !task.run_if_ready() {
                continue;
            }

            processor.switch_to_task(&task);

            if task.state_running_to_ready() {
                push_task(task);
            }
        } else {
            arch::enable_interrupt();
            arch::wait_for_interrupt();
        }
    }
}
