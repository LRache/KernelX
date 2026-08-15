use alloc::sync::Arc;
use alloc::vec::Vec;

use crate::kernel::errno::{Errno, SysResult};
use crate::kernel::event::{Event, FileEvent};
use crate::kernel::ipc::{KSiFields, SiCode, SiSigChld, SignalActionFlags, SignalNum, signum};
use crate::kernel::scheduler;
use crate::kernel::scheduler::current;
use crate::kernel::scheduler::tid::Tid;
use crate::kernel::task::manager;

use super::*;

#[derive(Debug, Clone, Copy)]
pub struct ChildWaitOptions {
    pub blocked: bool,
    pub wait_parent_tid: Option<Tid>,
    pub wait_exited: bool,
    pub wait_stopped: bool,
    pub wait_ptrace_stopped: bool,
    pub wait_continued: bool,
    pub consume: bool,
}

impl ChildWaitOptions {
    pub fn new(blocked: bool) -> Self {
        Self {
            blocked,
            wait_parent_tid: None,
            wait_exited: true,
            wait_stopped: false,
            wait_ptrace_stopped: false,
            wait_continued: false,
            consume: true,
        }
    }

    pub fn blocked(mut self, blocked: bool) -> Self {
        self.blocked = blocked;
        self
    }

    pub fn wait_parent_tid(mut self, wait_parent_tid: Option<Tid>) -> Self {
        self.wait_parent_tid = wait_parent_tid;
        self
    }

    pub fn wait_exited(mut self, wait_exited: bool) -> Self {
        self.wait_exited = wait_exited;
        self
    }

    pub fn wait_stopped(mut self, wait_stopped: bool) -> Self {
        self.wait_stopped = wait_stopped;
        self
    }

    pub fn wait_ptrace_stopped(mut self, wait_ptrace_stopped: bool) -> Self {
        self.wait_ptrace_stopped = wait_ptrace_stopped;
        self
    }

    pub fn wait_continued(mut self, wait_continued: bool) -> Self {
        self.wait_continued = wait_continued;
        self
    }

    pub fn consume(mut self, consume: bool) -> Self {
        self.consume = consume;
        self
    }
}

pub struct WaitResult {
    pub tid: Tid,
    pub pcb: Arc<PCB>,
    pub status: WaitStatus,
}

impl PCB {
    /// Wakes waiters that can observe a state change from the given child.
    pub fn wake_waiting_tasks(&self, child: Tid, wait_parent_tid: Tid) {
        let mut child_wait = self.child_wait.lock();
        let mut woken_tasks = Vec::new();
        let mut i = 0;
        while i < child_wait.waiters.len() {
            if child_wait.waiters[i]
                .1
                .map_or(true, |waiting_tid| waiting_tid == wait_parent_tid)
            {
                let (task, _) = child_wait.waiters.swap_remove(i);
                woken_tasks.push(task);
            } else {
                i += 1;
            }
        }
        drop(child_wait);

        woken_tasks.into_iter().for_each(|task| {
            let _ = scheduler::wakeup_task(task, Event::Process { child });
        });
    }

    /// Reports a ptrace stop to waiters and optionally sends SIGCHLD.
    pub fn notify_ptrace_stopped(&self, tid: Tid, uid: Uid, signum: SignalNum, tracer_tid: Tid) {
        self.wake_waiting_tasks(tid, tracer_tid);

        let send_sigchld = !self
            .signal
            .actions
            .lock()
            .get(signum::SIGCHLD)
            .flags
            .contains(SignalActionFlags::SA_NOCLDSTOP);

        if send_sigchld {
            let fields = KSiFields::SigChld(SiSigChld {
                si_pid: tid,
                si_uid: uid,
                si_status: signum.num() as i32,
                si_utime: 0,
                si_stime: 0,
            });
            self.send_signal(signum::SIGCHLD, SiCode::CLD_TRAPPED, 0, fields, None)
                .unwrap_or(());
        }
    }

    /// Notifies an explicit parent about a process-level waitable state.
    pub(super) fn notify_parent_wait_status(&self, parent: &PCB, wait_parent_tid: Tid, status: WaitStatus) {
        parent.wake_waiting_tasks(self.pid, wait_parent_tid);

        let (signal, si_code, si_status) = match status {
            WaitStatus::Exited(status) => (self.exit_signal, SiCode::SI_KERNEL, status.si_status()),
            WaitStatus::Stopped(signum) => {
                let send_sigchld = !parent
                    .signal
                    .actions
                    .lock()
                    .get(signum::SIGCHLD)
                    .flags
                    .contains(SignalActionFlags::SA_NOCLDSTOP);
                if !send_sigchld {
                    return;
                }
                (signum::SIGCHLD, SiCode::CLD_STOPPED, signum.num() as i32)
            }
            WaitStatus::Continued => {
                let send_sigchld = !parent
                    .signal
                    .actions
                    .lock()
                    .get(signum::SIGCHLD)
                    .flags
                    .contains(SignalActionFlags::SA_NOCLDSTOP);
                if !send_sigchld {
                    return;
                }
                (signum::SIGCHLD, SiCode::CLD_CONTINUED, signum::SIGCONT.num() as i32)
            }
            WaitStatus::PtraceStopped(_) => unreachable!("ptrace stops are notified through the tracer"),
        };

        if signal.is_empty() {
            return;
        }

        let fields = KSiFields::SigChld(SiSigChld {
            si_pid: self.pid,
            si_uid: self.uid(),
            si_status,
            si_utime: 0,
            si_stime: 0,
        });
        parent.send_signal(signal, si_code, 0, fields, None).unwrap_or(());
    }

    /// Records a job-control stop and notifies the parent process.
    pub fn notify_stopped(&self, signum: SignalNum) {
        let notification_target = {
            let parent = self.parent.lock();
            let mut child_wait_status = self.child_wait_status.lock();
            if matches!(*child_wait_status, Some(ChildWaitStatus::Stopped { .. })) {
                None
            } else {
                *child_wait_status = Some(ChildWaitStatus::Stopped {
                    signum,
                    reported: false,
                });
                parent.as_ref().map(|parent| (parent.clone(), self.wait_parent_tid()))
            }
        };

        if let Some((parent, wait_parent_tid)) = notification_target {
            self.notify_parent_wait_status(&parent, wait_parent_tid, WaitStatus::Stopped(signum));
        }
    }

    /// Records a continued state and notifies the parent process.
    pub fn notify_continued(&self) {
        let notification_target = {
            let parent = self.parent.lock();
            *self.child_wait_status.lock() = Some(ChildWaitStatus::Continued { reported: false });
            parent.as_ref().map(|parent| (parent.clone(), self.wait_parent_tid()))
        };

        if let Some((parent, wait_parent_tid)) = notification_target {
            self.notify_parent_wait_status(&parent, wait_parent_tid, WaitStatus::Continued);
        }
    }

    /// Registers or completes a pidfd read-ready wait.
    pub fn wait_pidfd_event(&self, waker: usize, event: FileEvent) -> Option<FileEvent> {
        let mut waiters = self.pidfd_waiters.lock();

        if self.is_exited() {
            return event.contains(FileEvent::READ_READY).then_some(FileEvent::READ_READY);
        }

        if event.contains(FileEvent::READ_READY) {
            waiters.wait_pending(
                current::task().clone(),
                Event::Poll {
                    event: FileEvent::READ_READY,
                    waker,
                },
            );
        }

        None
    }

    /// Cancels the current task's pending pidfd wait.
    pub fn wait_pidfd_event_cancel(&self) {
        self.pidfd_waiters.lock().remove(current::task());
    }

    /// Collects traced tasks visible to the current waiter.
    fn traced_tasks(&self, options: ChildWaitOptions) -> Vec<Arc<TCB>> {
        manager::tcbs()
            .lock()
            .values()
            .filter(|task| task.is_traced_by_waiter(current::pcb(), options.wait_parent_tid))
            .cloned()
            .collect()
    }

    /// Returns a wait result for a single ptrace task if it is reportable.
    fn waitable_ptrace_task(&self, task: Arc<TCB>, options: ChildWaitOptions) -> Option<WaitResult> {
        if !options.wait_ptrace_stopped {
            return None;
        }

        task.ptrace_wait_status(current::pcb(), options.consume)
            .map(|status| WaitResult {
                tid: task.tid(),
                pcb: task.parent().clone(),
                status,
            })
    }

    /// Finds a visible ptrace task matching the requested selector.
    fn waitable_ptrace_task_matching(
        &self,
        options: ChildWaitOptions,
        matches: impl Fn(&TCB) -> bool,
    ) -> (bool, Option<WaitResult>) {
        if !options.wait_ptrace_stopped {
            return (false, None);
        }

        let mut has_matching_task = false;

        for task in self.traced_tasks(options) {
            if !matches(&task) {
                continue;
            }
            has_matching_task = true;
            if let Some(result) = self.waitable_ptrace_task(task, options) {
                return (true, Some(result));
            }
        }

        (has_matching_task, None)
    }

    /// Checks whether a child has an unreported state requested by options.
    fn child_has_waitable_status(child: &PCB, options: ChildWaitOptions) -> bool {
        if options.wait_exited && child.is_exited() {
            return true;
        }

        match *child.child_wait_status.lock() {
            Some(ChildWaitStatus::Stopped { reported: false, .. }) => options.wait_stopped,
            Some(ChildWaitStatus::Continued { reported: false }) => options.wait_continued,
            _ => false,
        }
    }

    /// Returns a parent-visible state that still needs to follow a reparented child.
    pub(super) fn pending_parent_wait_status(&self) -> Option<WaitStatus> {
        if let State::Exited(status) = *self.state.lock() {
            return Some(WaitStatus::Exited(status));
        }

        match *self.child_wait_status.lock() {
            Some(ChildWaitStatus::Stopped {
                signum,
                reported: false,
            }) => Some(WaitStatus::Stopped(signum)),
            Some(ChildWaitStatus::Continued { reported: false }) => Some(WaitStatus::Continued),
            _ => None,
        }
    }

    /// Finds a selected child and the first selected child with waitable state.
    fn matching_child_in(
        children: &[Arc<PCB>],
        options: ChildWaitOptions,
        matches: impl Fn(&PCB) -> bool,
    ) -> (bool, Option<Arc<PCB>>) {
        let mut has_matching_child = false;
        let mut waitable_child = None;

        for child in children {
            if !matches(child)
                || options
                    .wait_parent_tid
                    .is_some_and(|tid| child.wait_parent_tid() != tid)
            {
                continue;
            }
            has_matching_child = true;
            if Self::child_has_waitable_status(child, options) {
                waitable_child = Some(child.clone());
                break;
            }
        }

        (has_matching_child, waitable_child)
    }

    /// Finds a selected child and the first selected child with waitable state.
    fn matching_child(&self, options: ChildWaitOptions, matches: impl Fn(&PCB) -> bool) -> (bool, Option<Arc<PCB>>) {
        let child_wait = self.child_wait.lock();
        Self::matching_child_in(&child_wait.children, options, matches)
    }

    /// Recycles an exited child and accounts its accumulated CPU usage.
    fn reap_child(&self, child: Arc<PCB>) -> Option<(Arc<PCB>, ExitStatus)> {
        let status = child.recycle()?;
        self.accumulate_waited_child(&child);
        self.child_wait.lock().children.retain(|c| c.pid() != child.pid());
        Some((child, status))
    }

    /// Returns the exit status without consuming the child.
    fn exit_wait_status(&self) -> Option<ExitStatus> {
        match *self.state.lock() {
            State::Exited(status) => Some(status),
            _ => None,
        }
    }

    /// Returns the first waitable state for a child process.
    fn waitable_child(&self, child: Arc<PCB>, options: ChildWaitOptions) -> Option<WaitResult> {
        if options.wait_exited {
            if options.consume {
                if let Some((child, status)) = self.reap_child(child.clone()) {
                    return Some(WaitResult {
                        tid: child.pid(),
                        pcb: child,
                        status: WaitStatus::Exited(status),
                    });
                }
            } else if let Some(status) = child.exit_wait_status() {
                return Some(WaitResult {
                    tid: child.pid(),
                    pcb: child,
                    status: WaitStatus::Exited(status),
                });
            }
        }

        if let Some(status) = child.signal_wait_status(options) {
            return Some(WaitResult {
                tid: child.pid(),
                pcb: child,
                status,
            });
        }

        None
    }

    /// Returns a stopped or continued wait status for this child.
    fn signal_wait_status(&self, options: ChildWaitOptions) -> Option<WaitStatus> {
        let mut child_wait_status = self.child_wait_status.lock();
        let status = child_wait_status.as_mut()?;
        match status {
            ChildWaitStatus::Stopped { signum, reported } if options.wait_stopped && !*reported => {
                if options.consume {
                    *reported = true;
                }
                Some(WaitStatus::Stopped(*signum))
            }
            ChildWaitStatus::Continued { reported } if options.wait_continued && !*reported => {
                if options.consume {
                    *reported = true;
                }
                Some(WaitStatus::Continued)
            }
            _ => None,
        }
    }

    /// Blocks the current task until a matching child state may have changed.
    fn current_wait_for_child_state_change(
        &self,
        reason: &'static str,
        options: ChildWaitOptions,
        has_waitable_state: impl Fn(&[Arc<PCB>]) -> bool,
    ) -> SysResult<()> {
        let task = current::task().clone();
        {
            let mut child_wait = self.child_wait.lock();
            if has_waitable_state(&child_wait.children) {
                return Ok(());
            }
            task.block(reason);
            child_wait.waiters.push((task, options.wait_parent_tid));
        }

        current::schedule();
        let event = current::task().take_wakeup_event().unwrap();
        match event {
            Event::Process { .. } => Ok(()),
            Event::Signal => {
                // The waiter may still be queued if the sleep is interrupted by a signal.
                self.child_wait
                    .lock()
                    .waiters
                    .retain(|(task, _)| !Arc::ptr_eq(task, current::task()));
                Err(Errno::EINTR)
            }
            _ => unreachable!("Unexpected event in wait: {:?}", event),
        }
    }

    /// Waits for a specific child pid or traced tid.
    pub fn wait_child(&self, pid: i32, options: ChildWaitOptions) -> SysResult<Option<WaitResult>> {
        loop {
            let (has_matching_child, waitable_child) = self.matching_child(options, |child| child.pid() == pid);

            if let Some(child) = waitable_child {
                if let Some(result) = self.waitable_child(child, options) {
                    return Ok(Some(result));
                }
                continue;
            }

            let (has_matching_ptrace_task, waitable_ptrace_task) =
                self.waitable_ptrace_task_matching(options, |task| task.tid() == pid);
            if let Some(result) = waitable_ptrace_task {
                return Ok(Some(result));
            }

            if !has_matching_child && !has_matching_ptrace_task {
                return Err(Errno::ECHILD);
            }

            if !options.blocked {
                return Ok(None);
            }

            self.current_wait_for_child_state_change("wait_child", options, |children| {
                Self::matching_child_in(children, options, |child| child.pid() == pid)
                    .1
                    .is_some()
                    || self
                        .waitable_ptrace_task_matching(options.consume(false), |task| task.tid() == pid)
                        .1
                        .is_some()
            })?;
        }
    }

    /// Waits for any child process or visible traced task.
    pub fn wait_any_child(&self, options: ChildWaitOptions) -> SysResult<Option<WaitResult>> {
        loop {
            let (has_matching_child, waitable_child) = self.matching_child(options, |_| true);

            if let Some(child) = waitable_child {
                if let Some(result) = self.waitable_child(child, options) {
                    return Ok(Some(result));
                }
                continue;
            }

            let (has_matching_ptrace_task, waitable_ptrace_task) =
                self.waitable_ptrace_task_matching(options, |_| true);
            if let Some(result) = waitable_ptrace_task {
                return Ok(Some(result));
            }

            if !has_matching_child && !has_matching_ptrace_task {
                return Err(Errno::ECHILD);
            }

            if !options.blocked {
                return Ok(None);
            }

            self.current_wait_for_child_state_change("wait_any_child", options, |children| {
                Self::matching_child_in(children, options, |_| true).1.is_some()
                    || self
                        .waitable_ptrace_task_matching(options.consume(false), |_| true)
                        .1
                        .is_some()
            })?;
        }
    }

    /// Waits for a child process or traced task in the given process group.
    pub fn wait_child_by_pgid(&self, pgid: Tid, options: ChildWaitOptions) -> SysResult<Option<WaitResult>> {
        loop {
            let (has_matching_child, waitable_child) = self.matching_child(options, |child| child.pgid() == pgid);

            if let Some(child) = waitable_child {
                if let Some(result) = self.waitable_child(child, options) {
                    return Ok(Some(result));
                }
                continue;
            }

            let (has_matching_ptrace_task, waitable_ptrace_task) =
                self.waitable_ptrace_task_matching(options, |task| task.parent().pgid() == pgid);
            if let Some(result) = waitable_ptrace_task {
                return Ok(Some(result));
            }

            if !has_matching_child && !has_matching_ptrace_task {
                return Err(Errno::ECHILD);
            }

            if !options.blocked {
                return Ok(None);
            }

            self.current_wait_for_child_state_change("wait_child_by_pgid", options, |children| {
                Self::matching_child_in(children, options, |child| child.pgid() == pgid)
                    .1
                    .is_some()
                    || self
                        .waitable_ptrace_task_matching(options.consume(false), |task| task.parent().pgid() == pgid)
                        .1
                        .is_some()
            })?;
        }
    }
}
