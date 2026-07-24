use alloc::collections::BTreeMap;
use alloc::sync::Arc;
use core::sync::atomic::{AtomicUsize, Ordering};
use core::time::Duration;
use spin::mutex::SpinMutex;

use crate::kernel::scheduler::{Task, TaskRunState, Tid, current};

const WATCHDOG_TICK: Duration = Duration::from_secs(1);
const WATCHDOG_THRESHOLD_TICKS: u64 = 12;
const WATCHDOG_REPORT_ALIVE_TICKS: u64 = 15;

struct BlockedTask {
    task: Arc<dyn Task>,
    ticks: u64,
    reason: &'static str,
    // PERF_DEBUG(scheduler-time): Concrete lock/object name for attribution.
    name: Option<&'static str>,
}

static BLOCKED_TASKS: SpinMutex<BTreeMap<Tid, BlockedTask>> = SpinMutex::new(BTreeMap::new());

#[repr(align(64))]
struct CacheLineAlignedAtomicUsize(AtomicUsize);

static READY_TASKS: CacheLineAlignedAtomicUsize = CacheLineAlignedAtomicUsize(AtomicUsize::new(0));
static RUNNING_TASKS: CacheLineAlignedAtomicUsize = CacheLineAlignedAtomicUsize(AtomicUsize::new(0));
static BLOCKED_TASK_COUNT: CacheLineAlignedAtomicUsize = CacheLineAlignedAtomicUsize(AtomicUsize::new(0));

#[derive(Default)]
struct TaskCounts {
    ready: usize,
    running: usize,
    blocked: usize,
}

fn count_tasks() -> TaskCounts {
    TaskCounts {
        ready: READY_TASKS.0.load(Ordering::Relaxed),
        running: RUNNING_TASKS.0.load(Ordering::Relaxed),
        blocked: BLOCKED_TASK_COUNT.0.load(Ordering::Relaxed),
    }
}

fn counter(state: TaskRunState) -> Option<&'static AtomicUsize> {
    match state {
        TaskRunState::Ready => Some(&READY_TASKS.0),
        TaskRunState::Running => Some(&RUNNING_TASKS.0),
        TaskRunState::Blocked => Some(&BLOCKED_TASK_COUNT.0),
        TaskRunState::Other => None,
    }
}

pub fn register_task(state: TaskRunState) {
    if let Some(counter) = counter(state) {
        counter.fetch_add(1, Ordering::Relaxed);
    }
}

pub fn unregister_task(state: TaskRunState) {
    if let Some(counter) = counter(state) {
        let previous = counter.fetch_sub(1, Ordering::Relaxed);
        debug_assert!(previous > 0);
    }
}

pub fn transition_task(old: TaskRunState, new: TaskRunState) {
    if old == new {
        return;
    }
    unregister_task(old);
    register_task(new);
}

pub fn kwatchdog() {
    let mut watchdog_ticks = 0u64;
    loop {
        current::sleep(WATCHDOG_TICK);
        watchdog_ticks += 1;
        BLOCKED_TASKS.lock().iter_mut().for_each(|(tid, blocked)| {
            let _ = &blocked.task;
            blocked.ticks += 1;
            if blocked.ticks % WATCHDOG_THRESHOLD_TICKS == 0 {
                crate::kwarn!(
                    "Watchdog: Tid {} has been blocked for {} ticks, reason: {}",
                    tid,
                    blocked.ticks,
                    blocked.reason
                );
                // PERF_DEBUG(scheduler-time): Report the attributed lock/object.
                if let Some(name) = blocked.name {
                    crate::kwarn!("Blocked object name: {}", name);
                }

                #[cfg(feature = "backtrace")]
                {
                    // The watchdog table only tracks blocked tasks, so their saved context is not running.
                    let frame_pointer = unsafe { (*blocked.task.kcontext_ptr()).frame_pointer() };
                    crate::klib::backtrace::print_backtrace_from_fp(frame_pointer);
                }

                #[cfg(all(feature = "lockdep", feature = "backtrace"))]
                if let Some((name, bt)) = blocked.task.lockstate().waiting() {
                    crate::kwarn!("Task is waiting on lock: {}", name);
                    crate::kwarn!("Lock was last acquired at:");
                    crate::klib::backtrace::print_backtrace_chain(&bt);
                }
            }
        });
        #[cfg(feature = "scheduler-time-debug")]
        {
            if watchdog_ticks % WATCHDOG_THRESHOLD_TICKS == 0 {
                // PERF_DEBUG(scheduler-time): Periodic temporary performance dump.
                super::time_debug::dump();
            }
        }
        if watchdog_ticks % WATCHDOG_REPORT_ALIVE_TICKS == 0 {
            let counts = count_tasks();
            crate::kdebug!(
                "kwatchdog: alive, tasks: ready={}, running={}, blocked={}",
                counts.ready,
                counts.running,
                counts.blocked
            );
        }
    }
}

// PERF_DEBUG(scheduler-time): name is temporary attribution metadata.
pub fn add_blocked_task(task: Arc<dyn Task>, reason: &'static str, name: Option<&'static str>) {
    let tid = task.tid();
    BLOCKED_TASKS.lock().insert(
        tid,
        BlockedTask {
            task,
            ticks: 0,
            reason,
            name,
        },
    );
}

pub fn remove_blocked_task(tid: Tid) {
    BLOCKED_TASKS.lock().remove(&tid);
}
