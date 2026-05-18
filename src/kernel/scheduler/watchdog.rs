use alloc::collections::BTreeMap;
use alloc::sync::Arc;
use core::time::Duration;
use spin::mutex::SpinMutex;

use crate::kernel::scheduler::{Task, Tid, current};

const WATCHDOG_INTERVAL: Duration = Duration::from_secs(1);

static BLOCKED_TASKS: SpinMutex<BTreeMap<Tid, (Arc<dyn Task>, u32)>> = SpinMutex::new(BTreeMap::new());

pub fn kwatchdog() {
    loop {
        BLOCKED_TASKS.lock().iter_mut().for_each(|(tid, (task, ticks))| {
            let _ = task;
            *ticks += 1;
            if *ticks >= 3 {
                crate::kwarn!("Watchdog: Tid {} has been blocked for {} ticks", tid, ticks);

                #[cfg(feature = "backtrace")]
                crate::klib::backtrace::print_backtrace_from_fp(task.kcontext().frame_pointer());

                #[cfg(feature = "deadlock-detect")]
                if let Some((name, bt)) = task.lockstate().waiting() {
                    crate::kwarn!("Task is waiting on lock: {}", name);
                    crate::kwarn!("Lock was last acquired at:");
                    crate::klib::backtrace::print_backtrace_chain(&bt);
                }

                *ticks = 0;
            }
        });

        current::sleep(WATCHDOG_INTERVAL);
    }
}

pub fn add_blocked_task(task: Arc<dyn Task>) {
    let tid = task.tid();
    BLOCKED_TASKS.lock().insert(tid, (task, 0));
}

pub fn remove_blocked_task(tid: Tid) {
    BLOCKED_TASKS.lock().remove(&tid);
}
