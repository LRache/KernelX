use core::time::Duration;

use crate::kernel::event::timer;

use super::*;

impl PCB {
    /// Returns cumulative CPU time of all waited-for children (and their descendants).
    pub fn children_usage_time(&self) -> (Duration, Duration) {
        *self.waited_children_time_usage.lock()
    }

    pub fn add_task_time(&self, user_time: Duration, system_time: Duration) {
        let mut usage = self.tasks_time_usage.lock();
        usage.0 += user_time;
        usage.1 += system_time;
    }

    /// Called by the parent after successfully reaping `child` via wait.
    /// Accumulates the child's own exit time and its waited-children time into self.
    pub(super) fn accumulate_waited_child(&self, child: &PCB) {
        let (child_u, child_s) = *child.tasks_time_usage_capture.lock();
        let (desc_u, desc_s) = *child.waited_children_time_usage.lock();
        let mut wct = self.waited_children_time_usage.lock();
        wct.0 += child_u + desc_u;
        wct.1 += child_s + desc_s;
    }

    pub fn tasks_usage_time(&self) -> (Duration, Duration) {
        // Read the folded accumulator BEFORE the per-thread residuals: a
        // concurrent context-switch fold moves time from a thread's residual
        // into the accumulator, and this order can only transiently
        // undercount — never double count.
        let (mut utime, mut stime) = *self.tasks_time_usage.lock();
        let now = timer::now();

        let tasks = self.tasks.lock();
        for tcb in tasks.iter() {
            let counter = tcb.time_counter.lock();
            let (unfolded_user, unfolded_system) = counter.unfolded();
            utime += unfolded_user;
            stime += unfolded_system;
            // Open intervals belong to threads currently on a CPU.
            if let Some(user_start) = counter.user_start {
                utime += now.checked_sub(user_start).unwrap_or(Duration::ZERO);
            }
            if let Some(system_start) = counter.system_start {
                stime += now.checked_sub(system_start).unwrap_or(Duration::ZERO);
            }
        }

        (utime, stime)
    }

    pub fn process_cpu_time(&self) -> Duration {
        let (utime, stime) = self.tasks_usage_time();
        utime + stime
    }
}
