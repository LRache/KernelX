use alloc::collections::vec_deque::VecDeque;
use alloc::sync::Arc;

use crate::arch;
use crate::kernel::event::Event;
use crate::kernel::scheduler::task::Task;
#[cfg(feature = "watchdog")]
use crate::kernel::scheduler::watchdog;
use crate::kernel::scheduler::{WakeupAction, WakeupFailure, current};
use crate::klib::SpinLock;

use super::processor::Processor;
// PERF_DEBUG(scheduler-time): Temporary running-time accounting.
#[cfg(feature = "scheduler-time-debug")]
use super::time_debug;

struct SchedulerState {
    ready_queue: VecDeque<Arc<dyn Task>>,
    idle_harts: usize,
}

pub struct Scheduler {
    state: SpinLock<SchedulerState>,
}

impl Scheduler {
    const fn new() -> Self {
        Self {
            state: SpinLock::new(
                SchedulerState {
                    ready_queue: VecDeque::new(),
                    idle_harts: 0,
                },
                "Scheduler::state",
            ),
        }
    }

    fn push_task(&self, task: Arc<dyn Task>) {
        let target_hart_mask = {
            let current_hart = current::hart_id();
            let current_hart_mask = 1usize << current_hart;
            let mut state = self.state.lock();
            state.ready_queue.push_back(task);

            let idle_harts = state.idle_harts & !current_hart_mask;
            if idle_harts == 0 {
                None
            } else {
                let target_hart_mask = 1usize << idle_harts.trailing_zeros();
                state.idle_harts &= !target_hart_mask;
                Some(target_hart_mask)
            }
        };

        if let Some(target_hart_mask) = target_hart_mask {
            arch::send_ipi(target_hart_mask);
        }
    }

    fn fetch_next_task(&self, hart_id: usize) -> Option<Arc<dyn Task>> {
        let hart_mask = 1usize << hart_id;
        let mut state = self.state.lock();
        state.idle_harts &= !hart_mask;
        let task = state.ready_queue.pop_front();
        if task.is_none() {
            state.idle_harts |= hart_mask;
        }
        task
    }
}

static SCHEDULER: Scheduler = Scheduler::new();

pub fn push_task(task: Arc<dyn Task>) {
    SCHEDULER.push_task(task);
}

pub fn fetch_next_task(hart_id: usize) -> Option<Arc<dyn Task>> {
    SCHEDULER.fetch_next_task(hart_id)
}

pub fn block_task_uninterruptible(task: Arc<dyn Task>, reason: &'static str) {
    task.block_uninterruptible(reason);
    #[cfg(feature = "watchdog")]
    watchdog::add_blocked_task(task, reason, None);
}

// PERF_DEBUG(scheduler-time): Preserve the blocked lock/object name for
// temporary per-name blocked-time attribution.
pub fn block_task_uninterruptible_named(task: Arc<dyn Task>, reason: &'static str, name: &'static str) {
    task.block_uninterruptible(name);
    #[cfg(feature = "watchdog")]
    watchdog::add_blocked_task(task, reason, Some(name));
    #[cfg(not(feature = "watchdog"))]
    let _ = name;
}

pub fn wakeup_task(task: Arc<dyn Task>, event: Event) -> Result<(), WakeupFailure> {
    task.wakeup(event).map(|action| {
        if action == WakeupAction::Deferred {
            return;
        }
        push_task(task);
    })
}

pub fn wakeup_task_uninterruptible(task: Arc<dyn Task>, event: Event) -> bool {
    if let Some(action) = task.wakeup_uninterruptible(event) {
        #[cfg(feature = "watchdog")]
        watchdog::remove_blocked_task(task.tid());
        if action == WakeupAction::Enqueue {
            push_task(task);
        }
        true
    } else {
        false
    }
}

pub fn run_tasks(processor: &mut Processor) -> ! {
    current::set(processor);
    loop {
        arch::disable_interrupt();
        if let Some(task) = fetch_next_task(processor.hart_id()) {
            if !task.run_if_ready() {
                continue;
            }

            // TODO: What if the task is exited here?
            task.resume_system_time();
            // PERF_DEBUG(scheduler-time): Start temporary per-CPU running-time
            // accounting; cpu_id is also passed to the debug-aware finish_switch.
            #[cfg(feature = "scheduler-time-debug")]
            let cpu_id = {
                let cpu_id = processor.hart_id();
                time_debug::start_running(cpu_id, arch::get_time_us());
                cpu_id
            };
            processor.switch_from_idle(&task);
            // PERF_DEBUG(scheduler-time): Finish the temporary running interval
            // and provide its timestamp to blocked-time accounting.
            #[cfg(feature = "scheduler-time-debug")]
            let _now_us = {
                let now_us = arch::get_time_us();
                time_debug::finish_running(cpu_id, now_us);
                now_us
            };

            #[cfg(feature = "scheduler-block-reason-debug")]
            let should_requeue = task.finish_switch(cpu_id, _now_us);
            #[cfg(not(feature = "scheduler-block-reason-debug"))]
            let should_requeue = task.finish_switch(0, 0);

            if should_requeue {
                push_task(task);
            } else {
                // `Arc<dyn Task>` SHOULD NOT be dropped here.
                // debug_assert!(Arc::strong_count(&task) != 1)
            }
        } else {
            arch::wait_for_interrupt();
            arch::enable_interrupt();
        }
    }
}
