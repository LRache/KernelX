use alloc::string::String;
use alloc::sync::Arc;
use alloc::vec::Vec;
use core::time::Duration;

use crate::fs::file::RandomAccessFile;
use crate::fs::{Dentry, InodeOps, Mode, vfs};
use crate::kernel::errno::{Errno, SysResult};
use crate::kernel::event::{Event, FileEvent, WaitQueue};
use crate::kernel::ipc::{KSiFields, PendingSignalQueue, SiCode, SiSigChld, SignalActionTable, SignalNum, signum};
use crate::kernel::main::deinit;
use crate::kernel::scheduler;
use crate::kernel::scheduler::tid::Tid;
use crate::kernel::scheduler::{Task, current, tid};
use crate::kernel::task::def::TaskCloneFlags;
use crate::kernel::task::{self, manager, with_initpcb};
use crate::kernel::uapi::Uid;
use crate::klib::{SleepLock, SpinLock};

use super::tcb::TCB;

pub type Pid = Tid;

struct Signal {
    actions: SpinLock<SignalActionTable>,
    pending: SpinLock<PendingSignalQueue>,
}

#[derive(Debug, Clone, Copy)]
pub enum ExitStatus {
    /// Normal exit with exit code (from exit/exit_group syscall)
    Normal(u8),
    /// Killed by signal, with optional core dump flag
    Signal { sig: u8, coredump: bool },
}

impl ExitStatus {
    /// Encode as POSIX wait status (wstatus)
    pub fn as_wstatus(self) -> u32 {
        match self {
            ExitStatus::Normal(code) => (code as u32) << 8,
            ExitStatus::Signal { sig, coredump } => {
                let status = sig as u32 & 0x7f;
                if coredump { status | 0x80 } else { status }
            }
        }
    }

    pub fn si_status(self) -> i32 {
        match self {
            ExitStatus::Normal(code) => code as i32,
            ExitStatus::Signal { sig, .. } => sig as i32,
        }
    }
}

#[derive(Debug)]
enum State {
    Running,
    Exited(ExitStatus),
    Recycled,
}

pub struct PCB {
    pid: Tid,
    pub parent: SpinLock<Option<Arc<PCB>>>,
    state: SpinLock<State>,
    exec_path: SpinLock<String>,
    exec_inode: SpinLock<Option<Arc<dyn InodeOps>>>,

    pub tasks: SleepLock<Vec<Arc<TCB>>>,
    cwd: SpinLock<Arc<Dentry>>,
    umask: SpinLock<u16>,
    file_size_limit: SpinLock<(usize, usize)>,
    waiting_task: SpinLock<Vec<Arc<dyn Task>>>,
    pidfd_waiters: SpinLock<WaitQueue<Event>>,

    signal: Signal,

    children: SleepLock<Vec<Arc<PCB>>>,

    pub itimer_ids: SpinLock<[Option<u64>; 3]>,
    /// Absolute expiry time in microseconds for each itimer (0 = inactive)
    pub itimer_expiry_us: SpinLock<[u64; 3]>,
    /// Interval for repeating itimers
    pub itimer_interval: SpinLock<[Duration; 3]>,

    // TODO: 减少鉴权时候的数据拷贝。
    uid: SpinLock<Uid>,
    euid: SpinLock<Uid>,
    suid: SpinLock<Uid>,
    fsuid: SpinLock<Uid>,
    gid: SpinLock<Uid>,
    egid: SpinLock<Uid>,
    sgid: SpinLock<Uid>,
    fsgid: SpinLock<Uid>,
    supplementary_gids: SpinLock<Vec<Uid>>,

    pgid: SpinLock<Pid>,
    sid: SpinLock<Pid>,
    execed: SpinLock<bool>,
    exit_signal: SignalNum,

    /// CPU time snapshot taken at exit (own threads). Preserved after recycle clears tasks.
    tasks_time_usage_capture: SpinLock<(Duration, Duration)>,
    /// Cumulative CPU time of all waited-for (reaped) children, including their descendants.
    waited_children_time_usage: SpinLock<(Duration, Duration)>,
}

impl PCB {
    pub fn new(pid: i32, pgid: Pid, parent: &Arc<PCB>, exit_signal: SignalNum) -> Arc<Self> {
        let exec_inode = parent.exec_inode.lock().clone();
        if let Some(inode) = exec_inode.as_ref() {
            inode.increment_exec_count();
        }

        Arc::new(Self {
            pid,
            parent: SpinLock::new(Some(parent.clone()), "PCB::parent"),
            state: SpinLock::new(State::Running, "PCB::state"),
            exec_path: SpinLock::new(parent.exec_path.lock().clone(), "PCB::exec_path"),
            exec_inode: SpinLock::new(exec_inode, "PCB::exec_inode"),

            tasks: SleepLock::new(Vec::new(), "PCB::tasks"),
            cwd: SpinLock::new(parent.cwd.lock().clone(), "PCB::cwd"),
            umask: SpinLock::new(*parent.umask.lock(), "PCB::umask"),
            file_size_limit: SpinLock::new(*parent.file_size_limit.lock(), "PCB::file_size_limit"),
            waiting_task: SpinLock::new(Vec::new(), "PCB::waiting_task"),
            pidfd_waiters: SpinLock::new(WaitQueue::new(), "PCB::pidfd_waiters"),

            signal: Signal {
                actions: SpinLock::new(parent.signal.actions.lock().clone(), "PCB::signal.actions"),
                pending: SpinLock::new(PendingSignalQueue::new(), "PCB::signal.pending"),
            },

            children: SleepLock::new(Vec::new(), "PCB::children"),

            itimer_ids: SpinLock::new([None; 3], "PCB::itimer_ids"),
            itimer_expiry_us: SpinLock::new([0; 3], "PCB::itimer_expiry_us"),
            itimer_interval: SpinLock::new([Duration::ZERO; 3], "PCB::itimer_interval"),

            uid: SpinLock::new(*parent.uid.lock(), "PCB::uid"),
            euid: SpinLock::new(*parent.euid.lock(), "PCB::euid"),
            suid: SpinLock::new(*parent.suid.lock(), "PCB::suid"),
            fsuid: SpinLock::new(*parent.fsuid.lock(), "PCB::fsuid"),
            gid: SpinLock::new(*parent.gid.lock(), "PCB::gid"),
            egid: SpinLock::new(*parent.egid.lock(), "PCB::egid"),
            sgid: SpinLock::new(*parent.sgid.lock(), "PCB::sgid"),
            fsgid: SpinLock::new(*parent.fsgid.lock(), "PCB::fsgid"),
            supplementary_gids: SpinLock::new(parent.supplementary_gids.lock().clone(), "PCB::supplementary_gids"),

            pgid: SpinLock::new(pgid, "PCB::pgid"),
            sid: SpinLock::new(parent.sid(), "PCB::sid"),
            execed: SpinLock::new(false, "PCB::execed"),
            exit_signal,

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

        let cwd = vfs::load_dentry(cwd)?;

        let pcb = Arc::new(Self {
            pid: new_tid,
            parent: SpinLock::new(None, "PCB::parent"),
            state: SpinLock::new(State::Running, "PCB::state"),
            exec_path: SpinLock::new(String::new(), "PCB::exec_path"),
            exec_inode: SpinLock::new(None, "PCB::exec_inode"),

            tasks: SleepLock::new(Vec::new(), "PCB::tasks"),
            cwd: SpinLock::new(cwd.clone(), "PCB::cwd"),
            umask: SpinLock::new(0o022, "PCB::umask"),
            file_size_limit: SpinLock::new((usize::MAX, usize::MAX), "PCB::file_size_limit"),
            waiting_task: SpinLock::new(Vec::new(), "PCB::waiting_task"),
            pidfd_waiters: SpinLock::new(WaitQueue::new(), "PCB::pidfd_waiters"),

            signal: Signal {
                actions: SpinLock::new(SignalActionTable::new(), "PCB::signal.actions"),
                pending: SpinLock::new(PendingSignalQueue::new(), "PCB::signal.pending"),
            },

            children: SleepLock::new(Vec::new(), "static::initpcb::children"),

            uid: SpinLock::new(0, "PCB::uid"),
            euid: SpinLock::new(0, "PCB::euid"),
            suid: SpinLock::new(0, "PCB::suid"),
            fsuid: SpinLock::new(0, "PCB::fsuid"),
            gid: SpinLock::new(0, "PCB::gid"),
            egid: SpinLock::new(0, "PCB::egid"),
            sgid: SpinLock::new(0, "PCB::sgid"),
            fsgid: SpinLock::new(0, "PCB::fsgid"),
            supplementary_gids: SpinLock::new(Vec::new(), "PCB::supplementary_gids"),

            pgid: SpinLock::new(new_tid, "PCB::pgid"),
            sid: SpinLock::new(new_tid, "PCB::sid"),
            execed: SpinLock::new(false, "PCB::execed"),
            exit_signal: signum::SIGCHLD,

            itimer_ids: SpinLock::new([None; 3], "PCB::itimer_ids"),
            itimer_expiry_us: SpinLock::new([0; 3], "PCB::itimer_expiry_us"),
            itimer_interval: SpinLock::new([Duration::ZERO; 3], "PCB::itimer_interval"),

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

    pub fn pid(&self) -> Tid {
        self.pid
    }

    pub fn pgid(&self) -> Pid {
        *self.pgid.lock()
    }

    pub fn set_pgid(&self, pgid: Pid) {
        *self.pgid.lock() = pgid;
    }

    pub fn sid(&self) -> Pid {
        *self.sid.lock()
    }

    pub fn set_sid(&self, sid: Pid) {
        *self.sid.lock() = sid;
    }

    pub fn is_session_leader(&self) -> bool {
        self.sid() == self.pid()
    }

    pub fn has_execed(&self) -> bool {
        *self.execed.lock()
    }

    pub fn uid(&self) -> Uid {
        *self.uid.lock()
    }

    pub fn set_uid(&self, uid: Uid) {
        *self.uid.lock() = uid;
    }

    pub fn euid(&self) -> Uid {
        *self.euid.lock()
    }

    pub fn set_euid(&self, euid: Uid) {
        *self.euid.lock() = euid;
        self.set_fsuid(euid);
    }

    pub fn suid(&self) -> Uid {
        *self.suid.lock()
    }

    pub fn set_suid(&self, suid: Uid) {
        *self.suid.lock() = suid;
    }

    pub fn fsuid(&self) -> Uid {
        *self.fsuid.lock()
    }

    pub fn set_fsuid(&self, fsuid: Uid) {
        *self.fsuid.lock() = fsuid;
    }

    pub fn gid(&self) -> Uid {
        *self.gid.lock()
    }

    pub fn set_gid(&self, gid: Uid) {
        *self.gid.lock() = gid;
    }

    pub fn egid(&self) -> Uid {
        *self.egid.lock()
    }

    pub fn set_egid(&self, egid: Uid) {
        *self.egid.lock() = egid;
        self.set_fsgid(egid);
    }

    pub fn sgid(&self) -> Uid {
        *self.sgid.lock()
    }

    pub fn set_sgid(&self, sgid: Uid) {
        *self.sgid.lock() = sgid;
    }

    pub fn fsgid(&self) -> Uid {
        *self.fsgid.lock()
    }

    pub fn set_fsgid(&self, fsgid: Uid) {
        *self.fsgid.lock() = fsgid;
    }

    pub fn supplementary_gids(&self) -> Vec<Uid> {
        self.supplementary_gids.lock().clone()
    }

    pub fn set_supplementary_gids(&self, gids: Vec<Uid>) {
        *self.supplementary_gids.lock() = gids;
    }

    pub fn exec_path(&self) -> String {
        self.exec_path.lock().clone()
    }

    pub fn is_exited(&self) -> bool {
        matches!(*self.state.lock(), State::Exited(_))
    }

    pub fn find_process(self: &Arc<Self>, pid: Tid) -> Option<Arc<Self>> {
        if self.pid() == pid {
            return Some(self.clone());
        }

        self.children
            .lock()
            .clone()
            .into_iter()
            .find_map(|child| child.find_process(pid))
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

    pub fn wait_for_all_tasks_exited_and_clear(&self) {
        loop {
            let mut tasks = self.tasks.lock();
            if let Some(task) = tasks.pop() {
                drop(tasks);
                while !task.is_exited() {
                    current::schedule();
                }
            } else {
                break;
            }
        }
    }

    fn recycle(&self) -> Option<ExitStatus> {
        let mut state = self.state.lock();
        let status = match *state {
            State::Exited(status) => Some(status),
            _ => return None,
        };
        *state = State::Recycled;
        drop(state);
        self.wait_for_all_tasks_exited_and_clear();
        status
    }

    pub fn cwd(&self) -> Arc<Dentry> {
        self.cwd.lock().clone()
    }

    pub fn set_cwd(&self, dentry: &Arc<Dentry>) {
        *self.cwd.lock() = dentry.clone();
    }

    pub fn umask(&self) -> u16 {
        *self.umask.lock()
    }

    pub fn set_umask(&self, mask: u16) {
        *self.umask.lock() = mask & 0o777;
    }

    pub fn file_size_limit(&self) -> (usize, usize) {
        *self.file_size_limit.lock()
    }

    pub fn set_file_size_limit(&self, cur: usize, max: usize) {
        *self.file_size_limit.lock() = (cur, max);
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
            let new_pcb = PCB::new(new_tid, self.pgid(), &real_parent, exit_signal);
            new_tcb = tcb.new_clone(new_tid, &new_pcb, userstack, flags, tls)?;
            new_pcb.tasks.lock().push(new_tcb.clone());
            real_parent.children.lock().push(new_pcb);
        } else {
            let new_pcb = PCB::new(new_tid, self.pgid(), self, exit_signal);
            new_tcb = tcb.new_clone(new_tid, &new_pcb, userstack, flags, tls)?;
            new_pcb.tasks.lock().push(new_tcb.clone());
            self.children.lock().push(new_pcb);
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
            self.children.lock().iter().for_each(|child| {
                let _ = child.send_signal(signum::SIGKILL, SiCode::EMPTY, 0, KSiFields::Empty, None);
            });
            loop {
                if let Some(child) = self.children.lock().pop() {
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
            tcb.set_dead();
        });

        // NOTE: Dropping `tasks` here would release ownership of each TCB and
        // trigger their destructors, which may perform async I/O. That is not
        // permitted inside a scheduler context. Instead, we leave the TCBs alive
        // and defer their cleanup to when this process is waited on (e.g. waitpid),
        // at which point it is safe to reclaim the resources.
        drop(tasks);
        self.replace_exec_inode(None);

        *self.tasks_time_usage_capture.lock() = self.tasks_usage_time();
        *self.state.lock() = State::Exited(status);
        self.pidfd_waiters.lock().wake_all(|event| match event {
            Event::Poll { waker, .. } => Event::Poll {
                event: FileEvent::READ_READY,
                waker,
            },
            event => event,
        });

        if let Some(parent) = self.parent.lock().as_ref() {
            parent.waiting_task.lock().drain(..).for_each(|t| {
                let _ = scheduler::wakeup_task(t, Event::Process { child: self.pid });
            });

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

        with_initpcb(|init_process| {
            let mut children = self.children.lock();
            children.iter_mut().for_each(|c| {
                *c.parent.lock() = Some(init_process.clone());
            });
            init_process.children.lock().append(&mut children);
        });

        manager::remove(self.pid);
    }

    fn replace_exec_inode(&self, new_inode: Option<Arc<dyn InodeOps>>) {
        let old_inode = {
            let mut exec_inode = self.exec_inode.lock();
            core::mem::replace(&mut *exec_inode, new_inode)
        };

        if let Some(old_inode) = old_inode {
            old_inode.end_exec();
        }
    }

    pub fn wait_child(&self, pid: i32, blocked: bool) -> Result<Option<(Arc<PCB>, ExitStatus)>, Errno> {
        let child = {
            let children = self.children.lock();
            children.iter().find(|c| c.pid() == pid).cloned()
        };

        if let Some(child) = child {
            if let Some(status) = child.recycle() {
                self.accumulate_waited_child(&child);

                let mut children = self.children.lock();
                let positon = children.iter().position(|c| c.pid() == pid).unwrap();
                children.swap_remove(positon);

                return Ok(Some((child, status)));
            }

            if blocked {
                loop {
                    self.waiting_task.lock().push(current::task().clone());

                    let event = current::block("wait_child");
                    match event {
                        Event::Process { child } => {
                            if child == pid {
                                break;
                            }
                        }
                        Event::Signal => {
                            return Err(Errno::EINTR);
                        }
                        _ => {
                            unreachable!("Unexpected event in wait_child: {:?}", event);
                        }
                    }
                }

                let status = if let Some(status) = child.recycle() {
                    status
                } else {
                    return Err(Errno::ECHILD); // The child process was recycled by other waiters
                };

                self.accumulate_waited_child(&child);

                let mut children = self.children.lock();
                children.retain(|c| c.pid() != pid);

                return Ok(Some((child, status)));
            } else {
                return Ok(None);
            }
        } else {
            // No child found
            return Err(Errno::ECHILD);
        }
    }

    pub fn wait_any_child(&self, blocked: bool) -> SysResult<Option<(Arc<PCB>, ExitStatus)>> {
        if let Some(child) = {
            let mut children = self.children.lock();
            if children.is_empty() {
                return Err(Errno::ECHILD);
            }

            // children.iter().for_each(|t| crate::kinfo!("{}", t.pid()));

            if let Some(pos) = children.iter().position(|c| c.is_exited()) {
                Some(children.swap_remove(pos))
            } else {
                None
            }
        } {
            if let Some(status) = child.recycle() {
                self.accumulate_waited_child(&child);
                return Ok(Some((child, status)));
            }
        };

        if !blocked {
            return Ok(None);
        }

        loop {
            self.waiting_task.lock().push(current::task().clone());

            let event = current::block("wait_any_child");
            match event {
                Event::Process { child } => {
                    let pid = child;
                    let child = {
                        let mut children = self.children.lock();

                        if let Some(pos) = children.iter().position(|c| c.pid() == pid) {
                            children.swap_remove(pos)
                        } else {
                            continue; // The child process was recycled by other waiters
                        }
                    };
                    if let Some(status) = child.recycle() {
                        self.accumulate_waited_child(&child);
                        return Ok(Some((child, status)));
                    } else {
                        continue; // The child process was recycled by other waiters
                    }
                }
                Event::Signal => return Err(Errno::EINTR),
                _ => unreachable!(),
            }
        }
    }

    pub fn wait_child_by_pgid(&self, pgid: Tid, blocked: bool) -> SysResult<Option<(Arc<PCB>, ExitStatus)>> {
        if let Some(child) = {
            let mut children = self.children.lock();
            if !children.iter().any(|c| c.pgid() == pgid) {
                return Err(Errno::ECHILD);
            }

            if let Some(pos) = children.iter().position(|c| c.pgid() == pgid && c.is_exited()) {
                Some(children.swap_remove(pos))
            } else {
                None
            }
        } {
            if let Some(status) = child.recycle() {
                self.accumulate_waited_child(&child);
                return Ok(Some((child, status)));
            }
        };

        if !blocked {
            return Ok(None);
        }

        loop {
            self.waiting_task.lock().push(current::task().clone());

            let event = current::block("wait_child_by_pgid");
            match event {
                Event::Process { child } => {
                    let pid = child;
                    let child = {
                        let mut children = self.children.lock();

                        if let Some(pos) = children.iter().position(|c| c.pid() == pid && c.pgid() == pgid) {
                            children.swap_remove(pos)
                        } else {
                            continue;
                        }
                    };
                    if let Some(status) = child.recycle() {
                        self.accumulate_waited_child(&child);
                        return Ok(Some((child, status)));
                    } else {
                        continue;
                    }
                }
                Event::Signal => return Err(Errno::EINTR),
                _ => unreachable!(),
            }
        }
    }

    pub fn signal_actions(&self) -> &SpinLock<SignalActionTable> {
        &self.signal.actions
    }

    pub fn pending_signals(&self) -> &SpinLock<PendingSignalQueue> {
        &self.signal.pending
    }

    /// Returns cumulative CPU time of all waited-for children (and their descendants).
    pub fn children_usage_time(&self) -> (Duration, Duration) {
        *self.waited_children_time_usage.lock()
    }

    /// Called by the parent after successfully reaping `child` via wait.
    /// Accumulates the child's own exit time and its waited-children time into self.
    fn accumulate_waited_child(&self, child: &PCB) {
        let (child_u, child_s) = *child.tasks_time_usage_capture.lock();
        let (desc_u, desc_s) = *child.waited_children_time_usage.lock();
        let mut wct = self.waited_children_time_usage.lock();
        wct.0 += child_u + desc_u;
        wct.1 += child_s + desc_s;
    }

    pub fn tasks_usage_time(&self) -> (Duration, Duration) {
        let tasks = self.tasks.lock();
        let mut utime = Duration::ZERO;
        let mut stime = Duration::ZERO;

        tasks.iter().for_each(|task| {
            let counter = task.time_counter.lock();
            utime += counter.user_time;
            stime += counter.system_time;
        });

        (utime, stime)
    }
}

impl Drop for PCB {
    fn drop(&mut self) {
        let exec_inode = self.exec_inode.lock().take();
        if let Some(exec_inode) = exec_inode {
            exec_inode.end_exec();
        }
    }
}

unsafe impl Send for PCB {}
unsafe impl Sync for PCB {}
