use alloc::string::String;
use alloc::sync::Arc;
use alloc::vec::Vec;
use core::time::Duration;

use crate::fs::file::RandomAccessFile;
use crate::fs::{Inode, Mode, vfs};
use crate::kernel::errno::{Errno, SysResult};
use crate::kernel::event::{Event, FileEvent, TimerTable, WaitQueue, timer};
use crate::kernel::ipc::{KSiFields, PendingSignalQueue, SiCode, SiSigChld, SignalActionTable, SignalNum, signum};
use crate::kernel::main::deinit;
use crate::kernel::scheduler::{self, WakeupAction, current, tid};
use crate::kernel::task::def::TaskCloneFlags;
use crate::kernel::task::{self, manager, with_initpcb};
use crate::klib::{SleepLock, SpinLock};

use super::*;

impl PCB {
    pub fn new(
        pid: i32,
        pgid: Pid,
        parent: &Arc<PCB>,
        inherit: &Arc<PCB>,
        exit_signal: SignalNum,
        new_uts: bool,
        wait_parent_tid: Tid,
    ) -> Arc<Self> {
        let exec_inode = parent.exec_inode.lock().clone();
        if let Some(inode) = exec_inode.as_ref() {
            inode.increment_exec_count();
        }

        let uts = if new_uts {
            inherit.uts.fork()
        } else {
            inherit.uts.clone()
        };

        Arc::new(Self {
            pid,
            parent: SpinLock::new(Some(parent.clone()), "PCB::parent"),
            wait_parent_tid: SpinLock::new(wait_parent_tid, "PCB::wait_parent_tid"),
            state: SpinLock::new(State::Running, "PCB::state"),
            child_wait_status: SpinLock::new(None, "PCB::child_wait_status"),
            exec_path: SpinLock::new(parent.exec_path.lock().clone(), "PCB::exec_path"),
            exec_inode: SpinLock::new(exec_inode, "PCB::exec_inode"),

            tasks: SleepLock::new(Vec::new(), "PCB::tasks"),
            root: SpinLock::new(inherit.root.lock().clone(), "PCB::root"),
            cwd: SpinLock::new(inherit.cwd.lock().clone(), "PCB::cwd"),
            umask: SpinLock::new(*parent.umask.lock(), "PCB::umask"),
            file_size_limit: SpinLock::new(*parent.file_size_limit.lock(), "PCB::file_size_limit"),
            child_wait: SleepLock::new(ChildWaitState::new(), "PCB::child_wait"),
            pidfd_waiters: SpinLock::new(WaitQueue::new("PCB::pidfd_waiters"), "PCB::pidfd_waiters"),
            uts,

            signal: Signal {
                actions: SpinLock::new(parent.signal.actions.lock().clone(), "PCB::signal.actions"),
                pending: SpinLock::new(PendingSignalQueue::new(), "PCB::signal.pending"),
            },

            itimers: SpinLock::new([None; 3], "PCB::itimers"),
            timers: TimerTable::new(),

            uid: SpinLock::new(*parent.uid.lock(), "PCB::uid"),
            euid: SpinLock::new(*parent.euid.lock(), "PCB::euid"),
            suid: SpinLock::new(*parent.suid.lock(), "PCB::suid"),
            fsuid: SpinLock::new(*parent.fsuid.lock(), "PCB::fsuid"),
            gid: SpinLock::new(*parent.gid.lock(), "PCB::gid"),
            egid: SpinLock::new(*parent.egid.lock(), "PCB::egid"),
            sgid: SpinLock::new(*parent.sgid.lock(), "PCB::sgid"),
            fsgid: SpinLock::new(*parent.fsgid.lock(), "PCB::fsgid"),
            supplementary_gids: SpinLock::new(parent.supplementary_gids.lock().clone(), "PCB::supplementary_gids"),
            nice: SpinLock::new(*parent.nice.lock(), "PCB::nice"),
            capabilities: SpinLock::new(*parent.capabilities.lock(), "PCB::capabilities"),

            pgid: SpinLock::new(pgid, "PCB::pgid"),
            sid: SpinLock::new(parent.sid(), "PCB::sid"),
            execed: SpinLock::new(false, "PCB::execed"),
            exit_signal,

            tasks_time_usage: SpinLock::new((Duration::ZERO, Duration::ZERO), "PCB::tasks_time_usage"),
            tasks_time_usage_capture: SpinLock::new((Duration::ZERO, Duration::ZERO), "PCB::tasks_time_usage_capture"),
            waited_children_time_usage: SpinLock::new(
                (Duration::ZERO, Duration::ZERO),
                "PCB::waited_children_time_usage",
            ),
        })
    }

    pub fn new_initprocess(
        initpath: &str,
        cwd: &str,
        argv: &[&str],
        envp: &[&str],
        tty: &str,
    ) -> SysResult<(Arc<PCB>, Arc<TCB>)> {
        let new_tid = tid::alloc();

        let root = vfs::get_root_dentry().clone();
        let cwd = vfs::load_dentry(cwd)?;

        let pcb = Arc::new(Self {
            pid: new_tid,
            parent: SpinLock::new(None, "PCB::parent"),
            wait_parent_tid: SpinLock::new(0, "PCB::wait_parent_tid"),
            state: SpinLock::new(State::Running, "PCB::state"),
            child_wait_status: SpinLock::new(None, "PCB::child_wait_status"),
            exec_path: SpinLock::new(String::new(), "PCB::exec_path"),
            exec_inode: SpinLock::new(None, "PCB::exec_inode"),

            tasks: SleepLock::new(Vec::new(), "PCB::tasks"),
            root: SpinLock::new(root, "PCB::root"),
            cwd: SpinLock::new(cwd.clone(), "PCB::cwd"),
            umask: SpinLock::new(0o022, "PCB::umask"),
            file_size_limit: SpinLock::new((usize::MAX, usize::MAX), "PCB::file_size_limit"),
            child_wait: SleepLock::new(ChildWaitState::new(), "PCB::child_wait"),
            pidfd_waiters: SpinLock::new(WaitQueue::new("PCB::pidfd_waiters"), "PCB::pidfd_waiters"),
            uts: UtsNamespace::new(),

            signal: Signal {
                actions: SpinLock::new(SignalActionTable::new(), "PCB::signal.actions"),
                pending: SpinLock::new(PendingSignalQueue::new(), "PCB::signal.pending"),
            },

            uid: SpinLock::new(0, "PCB::uid"),
            euid: SpinLock::new(0, "PCB::euid"),
            suid: SpinLock::new(0, "PCB::suid"),
            fsuid: SpinLock::new(0, "PCB::fsuid"),
            gid: SpinLock::new(0, "PCB::gid"),
            egid: SpinLock::new(0, "PCB::egid"),
            sgid: SpinLock::new(0, "PCB::sgid"),
            fsgid: SpinLock::new(0, "PCB::fsgid"),
            supplementary_gids: SpinLock::new(Vec::new(), "PCB::supplementary_gids"),
            nice: SpinLock::new(0, "PCB::nice"),
            capabilities: SpinLock::new(ProcessCapabilities::init(), "PCB::capabilities"),

            pgid: SpinLock::new(new_tid, "PCB::pgid"),
            sid: SpinLock::new(new_tid, "PCB::sid"),
            execed: SpinLock::new(false, "PCB::execed"),
            exit_signal: signum::SIGCHLD,

            tasks_time_usage: SpinLock::new((Duration::ZERO, Duration::ZERO), "PCB::tasks_time_usage"),
            itimers: SpinLock::new([None; 3], "PCB::itimers"),
            timers: TimerTable::new(),

            tasks_time_usage_capture: SpinLock::new((Duration::ZERO, Duration::ZERO), "PCB::tasks_time_usage_capture"),
            waited_children_time_usage: SpinLock::new(
                (Duration::ZERO, Duration::ZERO),
                "PCB::waited_children_time_usage",
            ),
        });

        let (first_task, exec_path, exec_inode) = TCB::new_inittask(new_tid, &pcb, initpath, argv, envp, tty);
        pcb.tasks.lock().push(first_task.clone());
        *pcb.exec_path.lock() = exec_path;
        pcb.replace_exec_inode(Some(exec_inode));

        Ok((pcb, first_task))
    }

    pub fn leader(&self) -> Option<Arc<TCB>> {
        self.tasks.lock().get(0).cloned()
    }

    pub(super) fn recycle(&self) -> Option<ExitStatus> {
        let mut state = self.state.lock();
        let status = match *state {
            State::Exited(status) => Some(status),
            _ => return None,
        };
        *state = State::Recycled;
        drop(state);

        let mut tasks = self.tasks.lock();
        tasks.iter().for_each(|tcb| {
            manager::remove(tcb.tid());
        });
        tasks.clear();
        status
    }

    pub fn clone_task(
        self: &Arc<Self>,
        tcb: &TCB,
        userstack: usize,
        flags: &TaskCloneFlags,
        tls: Option<usize>,
        exit_signal: SignalNum,
    ) -> Result<Arc<TCB>, Errno> {
        let new_tid = tid::alloc();
        let new_tcb;

        if flags.thread {
            new_tcb = tcb.new_clone(new_tid, self, userstack, flags, tls)?;
            self.tasks.lock().push(new_tcb.clone());
        } else if flags.parent {
            // CLONE_PARENT: the new process shares the same parent as the caller
            let real_parent = self.parent.lock().clone().ok_or(Errno::EINVAL)?;
            let new_pcb = PCB::new(
                new_tid,
                self.pgid(),
                &real_parent,
                self,
                exit_signal,
                flags.new_uts,
                self.wait_parent_tid(),
            );
            new_tcb = tcb.new_clone(new_tid, &new_pcb, userstack, flags, tls)?;
            new_pcb.tasks.lock().push(new_tcb.clone());
            real_parent.child_wait.lock().children.push(new_pcb);
        } else {
            let new_pcb = PCB::new(new_tid, self.pgid(), self, self, exit_signal, flags.new_uts, tcb.tid());
            new_tcb = tcb.new_clone(new_tid, &new_pcb, userstack, flags, tls)?;
            new_pcb.tasks.lock().push(new_tcb.clone());
            self.child_wait.lock().children.push(new_pcb);
        }

        manager::insert(new_tcb.clone());

        Ok(new_tcb)
    }

    pub fn remove_task(&self, tcb: &TCB) {
        let mut tasks = self.tasks.lock();
        if let Some(pos) = tasks.iter().position(|t| t.tid() == tcb.tid()) {
            tasks.swap_remove(pos);
        }
    }

    pub fn exec(
        self: &Arc<Self>,
        tcb: &TCB,
        file: Arc<RandomAccessFile>,
        invoked_path: &str,
        argv: &[&str],
        envp: &[&str],
    ) -> SysResult<()> {
        let filemode = file.mode()?;
        let fileowner = file.owner()?;

        let (first_task, exec_path, exec_inode) = tcb.new_exec(file, invoked_path, argv, envp)?;

        {
            let mut tasks = self.tasks.lock();
            tasks.drain(..).for_each(|tcb| {
                tcb.set_dead();
                manager::remove(tcb.tid());
            });
            tasks.push(first_task.clone());
        }

        self.timers.clear();

        if filemode.contains(Mode::S_ISUID) {
            self.set_euid(fileowner.0);
            self.set_suid(fileowner.0);
        }
        if filemode.contains(Mode::S_ISGID) {
            self.set_egid(fileowner.1);
            self.set_sgid(fileowner.1);
        }

        self.signal.actions.lock().reset_for_exec();
        *self.execed.lock() = true;
        *self.exec_path.lock() = exec_path;
        self.replace_exec_inode(Some(exec_inode));

        scheduler::push_task(first_task.clone());
        manager::insert(first_task);

        Ok(())
    }

    pub fn exit(self: &Arc<Self>, status: ExitStatus) {
        // If the init process exits, run deinit and halt the system.
        // NOTE: `deinit()` may issue async I/O, so the task state MUST NOT
        // be set to `Exited` before `deinit()` returns — once the task is
        // marked exited it will never be rescheduled, causing it to block
        // indefinitely and leaving the system hung instead of halting cleanly.
        if self.pid == task::INIT_UTASK_TID {
            self.child_wait.lock().children.iter().for_each(|child| {
                let _ = child.send_signal(signum::SIGKILL, SiCode::EMPTY, 0, KSiFields::Empty, None);
            });
            loop {
                if let Some(child) = self.child_wait.lock().children.pop() {
                    loop {
                        if child.is_exited() {
                            child.recycle();
                            break;
                        }
                        current::schedule();
                    }
                } else {
                    break;
                }
            }
            deinit();
            panic!("Init process exited with status {:?}, system will halt.", status);
        }

        // crate::kinfo!("pcb {} exited with code {}", self.pid(), code);

        let tasks = self.tasks.lock();
        tasks.iter().for_each(|tcb| {
            tcb.fdtable().lock().close_all();
            tcb.set_dead();
            tcb.wake_parent_waiting_vfork();
            if tcb.tid() != self.pid {
                manager::remove(tcb.tid());
            }
            if let Some(action) = tcb.resume_from_stopped() {
                if action == WakeupAction::Enqueue {
                    scheduler::push_task(tcb.clone());
                }
            } else if let Ok(action) = tcb.resume_from_ptrace_stop(None) {
                if action == WakeupAction::Enqueue {
                    scheduler::push_task(tcb.clone());
                }
            } else {
                let _ = scheduler::wakeup_task(tcb.clone(), Event::Signal);
            }
        });

        // NOTE: Dropping `tasks` here would release ownership of each TCB and
        // trigger their destructors, which may perform async I/O. That is not
        // permitted inside a scheduler context. Instead, we leave the TCBs alive
        // and defer their cleanup to when this process is waited on (e.g. waitpid),
        // at which point it is safe to reclaim the resources.
        drop(tasks);
        self.timers.clear();
        self.itimers.lock().iter().for_each(|itimer| {
            if let Some(itimer) = itimer {
                timer::remove_timer(itimer.id);
            }
        });
        self.replace_exec_inode(None);

        *self.tasks_time_usage_capture.lock() = self.tasks_usage_time();
        *self.child_wait_status.lock() = None;
        *self.state.lock() = State::Exited(status);
        self.pidfd_waiters.lock().wake_all(|event| match event {
            Event::Poll { waker, .. } => Event::Poll {
                event: FileEvent::READ_READY,
                waker,
            },
            event => event,
        });

        if let Some(parent) = self.parent.lock().as_ref() {
            parent.wake_waiting_tasks(self.pid, self.wait_parent_tid());

            let fields = KSiFields::SigChld(SiSigChld {
                si_pid: self.pid,
                si_uid: current::uid(),
                si_status: status.si_status(),
                si_utime: 0,
                si_stime: 0,
            });
            if !self.exit_signal.is_empty() {
                parent
                    .send_signal(self.exit_signal, SiCode::SI_KERNEL, 0, fields, None)
                    .unwrap_or(());
            }
        }

        let mut children = {
            let mut child_wait = self.child_wait.lock();
            child_wait.children.split_off(0)
        };
        with_initpcb(|init_process| {
            children.iter_mut().for_each(|c| {
                *c.parent.lock() = Some(init_process.clone());
                c.set_wait_parent_tid(init_process.pid());
            });
            init_process.child_wait.lock().children.append(&mut children);
        });
    }

    fn replace_exec_inode(&self, new_inode: Option<Arc<Inode>>) {
        let old_inode = {
            let mut exec_inode = self.exec_inode.lock();
            core::mem::replace(&mut *exec_inode, new_inode)
        };

        if let Some(old_inode) = old_inode {
            old_inode.end_exec();
        }
    }
}

impl Drop for PCB {
    fn drop(&mut self) {
        self.timers.clear();
        let exec_inode = self.exec_inode.lock().take();
        if let Some(exec_inode) = exec_inode {
            exec_inode.end_exec();
        }
    }
}
