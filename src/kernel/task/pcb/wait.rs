use alloc::sync::Arc;

use crate::kernel::errno::{Errno, SysResult};
use crate::kernel::event::{Event, FileEvent};
use crate::kernel::ipc::{KSiFields, SiCode, SiSigChld, SignalActionFlags, SignalNum, signum};
use crate::kernel::scheduler::tid::Tid;
use crate::kernel::scheduler::{self, current};

use super::*;

#[derive(Debug, Clone, Copy)]
pub struct ChildWaitOptions {
    pub blocked: bool,
    pub wait_parent_tid: Option<Tid>,
    pub wait_exited: bool,
    pub wait_stopped: bool,
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

    pub fn wait_continued(mut self, wait_continued: bool) -> Self {
        self.wait_continued = wait_continued;
        self
    }

    pub fn consume(mut self, consume: bool) -> Self {
        self.consume = consume;
        self
    }
}

impl PCB {
    pub fn notify_stopped(&self, signum: SignalNum) {
        let should_notify = {
            let mut child_wait_status = self.child_wait_status.lock();
            if matches!(*child_wait_status, Some(ChildWaitStatus::Stopped { .. })) {
                false
            } else {
                *child_wait_status = Some(ChildWaitStatus::Stopped {
                    signum,
                    reported: false,
                });
                true
            }
        };

        if !should_notify {
            return;
        }

        if let Some(parent) = self.parent.lock().as_ref() {
            parent.waiting_task.lock().drain(..).for_each(|task| {
                let _ = scheduler::wakeup_task(task, Event::Process { child: self.pid });
            });

            let send_sigchld = !parent
                .signal
                .actions
                .lock()
                .get(signum::SIGCHLD)
                .flags
                .contains(SignalActionFlags::SA_NOCLDSTOP);

            if send_sigchld {
                let fields = KSiFields::SigChld(SiSigChld {
                    si_pid: self.pid,
                    si_uid: self.uid(),
                    si_status: signum.num() as i32,
                    si_utime: 0,
                    si_stime: 0,
                });
                parent
                    .send_signal(signum::SIGCHLD, SiCode::CLD_STOPPED, 0, fields, None)
                    .unwrap_or(());
            }
        }
    }

    pub fn notify_continued(&self) {
        *self.child_wait_status.lock() = Some(ChildWaitStatus::Continued { reported: false });

        if let Some(parent) = self.parent.lock().as_ref() {
            parent.waiting_task.lock().drain(..).for_each(|task| {
                let _ = scheduler::wakeup_task(task, Event::Process { child: self.pid });
            });

            let send_sigchld = !parent
                .signal
                .actions
                .lock()
                .get(signum::SIGCHLD)
                .flags
                .contains(SignalActionFlags::SA_NOCLDSTOP);

            if send_sigchld {
                let fields = KSiFields::SigChld(SiSigChld {
                    si_pid: self.pid,
                    si_uid: self.uid(),
                    si_status: signum::SIGCONT.num() as i32,
                    si_utime: 0,
                    si_stime: 0,
                });
                parent
                    .send_signal(signum::SIGCHLD, SiCode::CLD_CONTINUED, 0, fields, None)
                    .unwrap_or(());
            }
        }
    }

    pub fn wait_pidfd_event(&self, waker: usize, event: FileEvent) -> Option<FileEvent> {
        if self.is_exited() {
            return event.contains(FileEvent::READ_READY).then_some(FileEvent::READ_READY);
        }

        if event.contains(FileEvent::READ_READY) {
            self.pidfd_waiters.lock().wait(
                current::task().clone(),
                Event::Poll {
                    event: FileEvent::READ_READY,
                    waker,
                },
            );
        }

        None
    }

    pub fn wait_pidfd_event_cancel(&self) {
        self.pidfd_waiters.lock().remove(current::task());
    }

    fn remove_current_waiting_task(&self) {
        self.waiting_task
            .lock()
            .retain(|task| !Arc::ptr_eq(task, current::task()));
    }

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

    fn reap_child(&self, child: Arc<PCB>) -> Option<(Arc<PCB>, ExitStatus)> {
        let status = child.recycle()?;
        self.accumulate_waited_child(&child);
        self.children.lock().retain(|c| c.pid() != child.pid());
        Some((child, status))
    }

    fn exit_wait_status(&self) -> Option<ExitStatus> {
        match *self.state.lock() {
            State::Exited(status) => Some(status),
            _ => None,
        }
    }

    fn waitable_child(&self, child: Arc<PCB>, options: ChildWaitOptions) -> Option<(Arc<PCB>, WaitStatus)> {
        if options.wait_exited {
            if options.consume {
                if let Some((child, status)) = self.reap_child(child.clone()) {
                    return Some((child, WaitStatus::Exited(status)));
                }
            } else if let Some(status) = child.exit_wait_status() {
                return Some((child, WaitStatus::Exited(status)));
            }
        }

        if let Some(status) = child.signal_wait_status(options) {
            return Some((child, status));
        }

        None
    }

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

    fn wait_for_child_state_change(&self, reason: &'static str) -> SysResult<()> {
        self.waiting_task.lock().push(current::task().clone());

        let event = current::block(reason);
        match event {
            Event::Process { .. } => Ok(()),
            Event::Signal => {
                self.remove_current_waiting_task();
                Err(Errno::EINTR)
            }
            _ => unreachable!("Unexpected event in wait: {:?}", event),
        }
    }

    pub fn wait_child(&self, pid: i32, options: ChildWaitOptions) -> Result<Option<(Arc<PCB>, WaitStatus)>, Errno> {
        loop {
            let child = {
                let children = self.children.lock();
                children.iter().find(|c| c.pid() == pid).cloned()
            };

            let child = child.ok_or(Errno::ECHILD)?;
            if options
                .wait_parent_tid
                .is_some_and(|tid| child.wait_parent_tid() != tid)
            {
                return Err(Errno::ECHILD);
            }

            if let Some(result) = self.waitable_child(child, options) {
                return Ok(Some(result));
            }

            if !options.blocked {
                return Ok(None);
            }

            self.wait_for_child_state_change("wait_child")?;
        }
    }

    pub fn wait_any_child(&self, options: ChildWaitOptions) -> SysResult<Option<(Arc<PCB>, WaitStatus)>> {
        loop {
            let (has_matching_child, waitable_child) = {
                let children = self.children.lock();
                let mut has_matching_child = false;
                let mut waitable_child = None;

                for child in children.iter() {
                    if options
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
            };

            if !has_matching_child {
                return Err(Errno::ECHILD);
            }

            if let Some(child) = waitable_child {
                if let Some(result) = self.waitable_child(child, options) {
                    return Ok(Some(result));
                }
                continue;
            }

            if !options.blocked {
                return Ok(None);
            }

            self.wait_for_child_state_change("wait_any_child")?;
        }
    }

    pub fn wait_child_by_pgid(
        &self,
        pgid: Tid,
        options: ChildWaitOptions,
    ) -> SysResult<Option<(Arc<PCB>, WaitStatus)>> {
        loop {
            let (has_matching_child, waitable_child) = {
                let children = self.children.lock();
                let mut has_matching_child = false;
                let mut waitable_child = None;

                for child in children.iter() {
                    if child.pgid() != pgid
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
            };

            if !has_matching_child {
                return Err(Errno::ECHILD);
            }

            if let Some(child) = waitable_child {
                if let Some(result) = self.waitable_child(child, options) {
                    return Ok(Some(result));
                }
                continue;
            }

            if !options.blocked {
                return Ok(None);
            }

            self.wait_for_child_state_change("wait_child_by_pgid")?;
        }
    }
}
