use alloc::collections::vec_deque::VecDeque;
use alloc::sync::Arc;
use core::sync::atomic::{AtomicUsize, Ordering};
use spin::Mutex;

use crate::arch;
use crate::kernel::event::Event;
use crate::kernel::scheduler::task::Task;
#[cfg(feature = "watchdog")]
use crate::kernel::scheduler::watchdog;
use crate::kernel::scheduler::{WakeupAction, WakeupFailure, current};

use super::processor::Processor;
// PERF_DEBUG(scheduler-time): Temporary running-time accounting.
#[cfg(feature = "scheduler-time-debug")]
use super::time_debug;

pub struct Scheduler {
    ready_queue: Mutex<VecDeque<Arc<dyn Task>>>,
    /// Mirror of `ready_queue.len()`, maintained under the queue lock, so the
    /// timer tick and the idle loop can check for work without locking.
    ready_count: AtomicUsize,
}

/// Bitmask of harts parked in the idle `wfi` branch of `run_tasks`. Used to
/// target a wakeup IPI at exactly one idle hart when a task is enqueued.
static IDLE_HARTS: AtomicUsize = AtomicUsize::new(0);

impl Scheduler {
    const fn new() -> Self {
        Self {
            ready_queue: Mutex::new(VecDeque::new()),
            ready_count: AtomicUsize::new(0),
        }
    }

    fn push_task(&self, task: Arc<dyn Task>) {
        let mut ready_queue = self.ready_queue.lock();
        ready_queue.push_back(task);
        // SeqCst pairs with the idle loop: the idler publishes its IDLE_HARTS
        // bit BEFORE re-checking `ready_count`, and this store lands BEFORE
        // the enqueuer reads IDLE_HARTS below. So either the idler's re-check
        // sees this task, or `kick_idle_hart` sees the idler's bit and IPIs.
        self.ready_count.store(ready_queue.len(), Ordering::SeqCst);
        drop(ready_queue);

        kick_idle_hart();
    }

    fn fetch_next_task(&self) -> Option<Arc<dyn Task>> {
        let mut ready_queue = self.ready_queue.lock();
        let task = ready_queue.pop_front();
        self.ready_count.store(ready_queue.len(), Ordering::SeqCst);
        task
    }
}

static SCHEDULER: Scheduler = Scheduler::new();

/// Lock-free "is there anything to run" check for the timer tick and the
/// idle loop. May transiently disagree with the queue by one enqueue/dequeue.
pub fn has_ready_tasks() -> bool {
    SCHEDULER.ready_count.load(Ordering::SeqCst) != 0
}

/// Wake at most one idle hart (the lowest set bit, never the current hart)
/// after a task was enqueued. One IPI per enqueue bounds the IPI rate.
fn kick_idle_hart() {
    let mut idle = IDLE_HARTS.load(Ordering::SeqCst);
    if idle != 0 && current::has_processor() {
        idle &= !(1usize << current::hart_id());
    }
    if idle != 0 {
        arch::send_ipi(idle & idle.wrapping_neg());
    }
}

pub fn push_task(task: Arc<dyn Task>) {
    SCHEDULER.push_task(task);
}

pub fn fetch_next_task() -> Option<Arc<dyn Task>> {
    SCHEDULER.fetch_next_task()
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
        if let Some(task) = fetch_next_task() {
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
            // Publish idleness BEFORE the final empty-queue re-check to close
            // the lost-wakeup window: an enqueuer pushes first and reads
            // IDLE_HARTS afterwards (both SeqCst), so either the re-check
            // below observes the new task or the enqueuer observes this bit
            // and sends a wakeup IPI that terminates `wait_for_interrupt`.
            let idle_bit = 1usize << processor.hart_id();
            IDLE_HARTS.fetch_or(idle_bit, Ordering::SeqCst);
            if has_ready_tasks() {
                IDLE_HARTS.fetch_and(!idle_bit, Ordering::SeqCst);
                continue;
            }
            arch::enable_interrupt();
            arch::wait_for_interrupt();
            IDLE_HARTS.fetch_and(!idle_bit, Ordering::SeqCst);
        }
    }
}
