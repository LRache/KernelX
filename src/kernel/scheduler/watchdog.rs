use alloc::collections::BTreeMap;
use alloc::sync::Arc;
use core::time::Duration;
use spin::mutex::SpinMutex;

use crate::kernel::scheduler::{Task, Tid, current};

const WATCHDOG_TICK: Duration = Duration::from_secs(1);
const WATCHDOG_THRESHOLD_TICKS: u8 = 3;
const WATCHDOG_REPORT_ALIVE_TICKS: u8 = 60;

static BLOCKED_TASKS: SpinMutex<BTreeMap<Tid, (Arc<dyn Task>, u8, &'static str)>> = SpinMutex::new(BTreeMap::new());

pub fn kwatchdog() {
    let mut alive_ticks = 0u8;
    loop {
        current::sleep(WATCHDOG_TICK);
        BLOCKED_TASKS
            .lock()
            .iter_mut()
            .for_each(|(tid, (task, ticks, reason))| {
                let _ = task;
                *ticks += 1;
                if *ticks >= WATCHDOG_THRESHOLD_TICKS {
                    crate::kwarn!(
                        "Watchdog: Tid {} has been blocked for {} ticks, reason: {}",
                        tid,
                        ticks,
                        reason
                    );

                    #[cfg(feature = "backtrace")]
                    {
                        // The watchdog table only tracks blocked tasks, so their saved context is not running.
                        let frame_pointer = unsafe { (*task.kcontext_ptr()).frame_pointer() };
                        crate::klib::backtrace::print_backtrace_from_fp(frame_pointer);
                    }

                    #[cfg(all(feature = "lockdep", feature = "backtrace"))]
                    if let Some((name, bt)) = task.lockstate().waiting() {
                        crate::kwarn!("Task is waiting on lock: {}", name);
                        crate::kwarn!("Lock was last acquired at:");
                        crate::klib::backtrace::print_backtrace_chain(&bt);
                    }

                    *ticks = 0;
                }
            });
        alive_ticks = alive_ticks + 1;
        if alive_ticks >= WATCHDOG_REPORT_ALIVE_TICKS {
            crate::kdebug!("kwatchdog: alive");
            alive_ticks = 0;
        }
    }
}

pub fn add_blocked_task(task: Arc<dyn Task>, reason: &'static str) {
    let tid = task.tid();
    BLOCKED_TASKS.lock().insert(tid, (task, 0, reason));
}

pub fn remove_blocked_task(tid: Tid) {
    BLOCKED_TASKS.lock().remove(&tid);
}
