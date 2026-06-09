use alloc::collections::vec_deque::VecDeque;
use alloc::sync::Arc;
use spin::Mutex;

use crate::arch;
use crate::kernel::event::Event;
use crate::kernel::scheduler::task::Task;
#[cfg(feature = "watchdog")]
use crate::kernel::scheduler::watchdog;
use crate::kernel::scheduler::{WakeupFailure, current};

use super::processor::Processor;

pub struct Scheduler {
    ready_queue: Mutex<VecDeque<Arc<dyn Task>>>,
}

impl Scheduler {
    const fn new() -> Self {
        Self {
            ready_queue: Mutex::new(VecDeque::new()),
        }
    }

    fn push_task(&self, task: Arc<dyn Task>) {
        let mut ready_queue = self.ready_queue.lock();
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

pub fn block_task_uninterruptible(task: Arc<dyn Task>, reason: &'static str) {
    task.block_uninterruptible(reason);
    #[cfg(feature = "watchdog")]
    watchdog::add_blocked_task(task, reason);
}

pub fn wakeup_task(task: Arc<dyn Task>, event: Event) -> Result<(), WakeupFailure> {
    task.wakeup(event).map(|_| {
        push_task(task);
    })
}

pub fn wakeup_task_uninterruptible(task: Arc<dyn Task>, event: Event) -> bool {
    if task.wakeup_uninterruptible(event) {
        #[cfg(feature = "watchdog")]
        watchdog::remove_blocked_task(task.tid());
        push_task(task);
        true
    } else {
        false
    }
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

            // TODO: What if the task is exited here?
            processor.switch_to_task(&task);

            if task.state_running_to_ready() {
                push_task(task);
            } else {
                // `Arc<dyn Task>` SHOULD NOT be dropped here.
                // debug_assert!(Arc::strong_count(&task) != 1)
            }
        } else {
            arch::enable_interrupt();
            arch::wait_for_interrupt();
        }
    }
}
