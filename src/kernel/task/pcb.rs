use alloc::string::String;
use alloc::vec::Vec;
use alloc::sync::Arc;
use core::time::Duration;

use crate::kernel::errno::{Errno, SysResult};
use crate::kernel::main::deinit;
use crate::kernel::scheduler::tid::Tid;
use crate::kernel::task::def::TaskCloneFlags;
use crate::kernel::task::{self, manager, with_initpcb};
use crate::kernel::scheduler::{Task, current, tid};
use crate::kernel::scheduler;
use crate::kernel::event::Event;
use crate::kernel::ipc::{KSiFields, PendingSignalQueue, SiCode, SiSigChld, SignalActionTable, signum};
use crate::fs::file::File;
use crate::fs::vfs;
use crate::fs::Dentry;
use crate::klib::{SleepLock, SpinLock};

use super::tcb::TCB;

pub type Pid = Tid;

struct Signal {
    actions: SpinLock<SignalActionTable>,
    pending: SpinLock<PendingSignalQueue>,
}

#[derive(Debug)]
enum State {
    Running,
    Exited(u8),
    Recycled
}

pub struct PCB {
    pid: Tid,
    pub parent: SpinLock<Option<Arc<PCB>>>,
    state: SpinLock<State>,
    exec_path: SpinLock<String>,
    
    pub tasks: SleepLock<Vec<Arc<TCB>>>,
    cwd: SpinLock<Arc<Dentry>>,
    umask: SpinLock<u16>,
    waiting_task: SpinLock<Vec<Arc<dyn Task>>>,

    signal: Signal,

    children: SleepLock<Vec<Arc<PCB>>>,

    pub itimer_ids: SpinLock<[Option<u64>; 3]>,
}

impl PCB {
    pub fn new(pid: i32, parent: &Arc<PCB>) -> Arc<Self> {
        Arc::new(Self {
            pid,
            parent: SpinLock::new(Some(parent.clone()), "PCB::parent"),
            state: SpinLock::new(State::Running, "PCB::state"),
            exec_path: SpinLock::new(parent.exec_path.lock().clone(), "PCB::exec_path"),

            tasks: SleepLock::new(Vec::new(), "PCB::tasks"),
            cwd: SpinLock::new(parent.cwd.lock().clone(), "PCB::cwd"),
            umask: SpinLock::new(*parent.umask.lock(), "PCB::umask"),
            waiting_task: SpinLock::new(Vec::new(), "PCB::waiting_task"),

            signal: Signal {
                actions: SpinLock::new(SignalActionTable::new(), "PCB::signal.actions"),
                pending: SpinLock::new(PendingSignalQueue::new(), "PCB::signal.pending"),
            },

            children: SleepLock::new(Vec::new(), "PCB::children"),

            itimer_ids: SpinLock::new([None; 3], "PCB::itimer_ids"),
        })
    }

    pub fn new_initprocess(initpath: &str, cwd: &str, argv: &[&str], envp: &[&str], tty: &str) -> SysResult<(Arc<PCB>, Arc<TCB>)> {
        let new_tid = tid::alloc();

        let cwd = vfs::load_dentry(cwd)?;

        let pcb = Arc::new(Self {
            pid: new_tid,
            parent: SpinLock::new(None, "PCB::parent"),
            state: SpinLock::new(State::Running, "PCB::state"),
            exec_path: SpinLock::new(String::new(), "PCB::exec_path"),

            tasks: SleepLock::new(Vec::new(), "PCB::tasks"),
            cwd: SpinLock::new(cwd.clone(), "PCB::cwd"),
            umask: SpinLock::new(0o022, "PCB::umask"),
            waiting_task: SpinLock::new(Vec::new(), "PCB::waiting_task"),

            signal: Signal {
                actions: SpinLock::new(SignalActionTable::new(), "PCB::signal.actions"),
                pending: SpinLock::new(PendingSignalQueue::new(), "PCB::signal.pending"),
            },

            children: SleepLock::new(Vec::new(), "static::initpcb::children"),

            itimer_ids: SpinLock::new([None; 3], "PCB::itimer_ids"),
        });

        let (first_task, exec_path) = TCB::new_inittask(new_tid, &pcb, initpath, argv, envp, tty);
        pcb.tasks.lock().push(first_task.clone());
        *pcb.exec_path.lock() = exec_path;

        Ok((pcb, first_task))
    }

    pub fn pid(&self) -> Tid {
        self.pid
    }

    pub fn exec_path(&self) -> String {
        self.exec_path.lock().clone()
    }

    pub fn is_exited(&self) -> bool {
         matches!(*self.state.lock(), State::Exited(_))
    }

    fn recycle(&self) -> Option<u8> {
        let mut state = self.state.lock();
        let code = match *state {
            State::Exited(code) => Some(code),
            _ => return None,
        };
        *state = State::Recycled;
        drop(state);
        let mut tasks = self.tasks.lock();
        tasks.iter().for_each(|tcb| {
            while !tcb.is_exited() {
                current::schedule();
            }
        });
        tasks.clear(); // Drop all TCBs to release their resources
        code
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

    pub fn clone_task(
        self: &Arc<Self>, 
        tcb: &TCB, 
        userstack: usize,
        flags: &TaskCloneFlags,
        tls: Option<usize>,
    ) -> Result<Arc<TCB>, Errno> {
        let new_tid = tid::alloc();
        let new_tcb;

        if flags.thread {
            new_tcb = tcb.new_clone(new_tid, self, userstack, flags, tls);
            self.tasks.lock().push(new_tcb.clone());
        } else {
            let new_parent = PCB::new(new_tid, self);
            new_tcb = tcb.new_clone(new_tid, &new_parent, userstack, flags, tls);
            new_parent.tasks.lock().push(new_tcb.clone());
            self.children.lock().push(new_parent.clone());
        }

        manager::insert(new_tcb.clone());

        Ok(new_tcb)
    }

    pub fn exec(
        self: &Arc<Self>, 
        tcb: &TCB, 
        file: Arc<File>,
        argv: &[&str], 
        envp: &[&str]
    ) -> SysResult<()> {        
        let (first_task, exec_path) = tcb.new_exec(file, argv, envp)?;

        {
            let mut tasks = self.tasks.lock();
            tasks.drain(..).for_each(|tcb| {
                tcb.set_dead();
                manager::remove(tcb.tid());
            });
            tasks.push(first_task.clone());
        }

        *self.exec_path.lock() = exec_path;

        scheduler::push_task(first_task.clone());
        manager::insert(first_task);

        Ok(())
    }

    pub fn exit(self: &Arc<Self>, code: u8) {
        // If the init process exits, run deinit and halt the system.
        // NOTE: `deinit()` may issue async I/O, so the task state MUST NOT
        // be set to `Exited` before `deinit()` returns — once the task is
        // marked exited it will never be rescheduled, causing it to block
        // indefinitely and leaving the system hung instead of halting cleanly.
        if self.pid == task::INIT_UTASK_TID {
            deinit();
            panic!("Init process exited with code {}, system will halt.", code);
        }
        
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

        *self.state.lock() = State::Exited(code);
        
        if let Some(parent) = self.parent.lock().as_ref() {
            parent.waiting_task.lock().drain(..).for_each(|t| {
                scheduler::wakeup_task(t, Event::Process { child: self.pid });
            });
            
            let fields = KSiFields::SigChld(SiSigChld { 
                si_pid: self.pid, 
                si_uid: current::uid(), 
                si_status: code as i32, 
                si_utime: 0,
                si_stime: 0
            });
            parent.send_signal(signum::SIGCHLD, SiCode::SI_KERNEL, fields, None).unwrap_or(());
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

    pub fn wait_child(&self, pid: i32, blocked: bool) -> Result<Option<u8>, Errno> {
        let child = {
            let children = self.children.lock();
            children.iter().find(|c| c.pid() == pid).cloned()
        };
        
        if let Some(child) = child {
            if let Some(exit_code) = child.recycle() {
                return Ok(Some(exit_code));
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
                        _ => unreachable!("Unexpected event in wait_child: {:?}", event),
                    }
                }
                
                let exit_code = if let Some(exit_code) = child.recycle() {
                    exit_code
                } else {
                    return Err(Errno::ECHILD); // The child process was recycled by other waiters
                };
                    
                let mut children = self.children.lock();
                children.retain(|c| c.pid() != pid);
                
                return Ok(Some(exit_code));
            } else {
                return Ok(None);
            }
        } else { // No child found
            if blocked {
                return Err(Errno::ECHILD);
            } else {
                return Ok(None);
            }
        }
    }

    pub fn wait_any_child(&self, blocked: bool) -> SysResult<Option<(i32, u8)>> {
        let mut children = self.children.lock();
    
        if let Some(child) = children.iter().find(|c| c.is_exited()) {
            let pid = child.pid();
            if let Some(exit_code) = child.recycle() {
                children.retain(|c| c.pid() != pid);
                return Ok(Some((pid, exit_code)));
            }
        }

        drop(children);
        
        if !blocked {
            return Ok(None);
        }

        self.waiting_task.lock().push(current::task().clone());

        loop {
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
                    if let Some(exit_code) = child.recycle() {
                        return Ok(Some((pid, exit_code)));
                    } else {
                        continue; // The child process was recycled by other waiters
                    }
                }
                Event::Signal => {
                    return Err(Errno::EINTR)
                }
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

unsafe impl Send for PCB {}
unsafe impl Sync for PCB {}
