use alloc::string::String;
use alloc::vec::Vec;
use alloc::sync::Arc;
use core::time::Duration;
use spin::Mutex;

use crate::kernel::errno::{Errno, SysResult};
use crate::kernel::scheduler::tid::Tid;
use crate::kernel::task::def::TaskCloneFlags;
use crate::kernel::task::{with_initprocess, manager};
use crate::kernel::scheduler::{Task, TaskState, current, tid};
use crate::kernel::scheduler;
use crate::kernel::event::Event;
use crate::kernel::ipc::{KSiFields, PendingSignalQueue, SiCode, SiSigChld, SignalActionTable, signum};
use crate::fs::file::{File, FileFlags};
use crate::fs::{Perm, PermFlags, vfs};
use crate::fs::Dentry;
use crate::klib::SpinLock;

use super::tcb::TCB;

pub type Pid = Tid;

struct Signal {
    actions: Mutex<SignalActionTable>,
    pending: Mutex<PendingSignalQueue>,
}

enum State {
    Running,
    Exited(u8),
    Dead,
}

pub struct PCB {
    pid: Tid,
    pub parent: SpinLock<Option<Arc<PCB>>>,
    state: SpinLock<State>,
    exec_path: SpinLock<String>,
    
    pub tasks: SpinLock<Vec<Arc<TCB>>>,
    cwd: SpinLock<Arc<Dentry>>,
    umask: SpinLock<u16>,
    waiting_task: SpinLock<Vec<Arc<dyn Task>>>,

    signal: Signal,

    children: Mutex<Vec<Arc<PCB>>>,

    pub itimer_ids: SpinLock<[Option<u64>; 3]>,
}

impl PCB {
    pub fn new(pid: i32, parent: &Arc<PCB>) -> Arc<Self> {
        Arc::new(Self {
            pid,
            parent: SpinLock::new(Some(parent.clone())),
            state: SpinLock::new(State::Running),
            exec_path: SpinLock::new(parent.exec_path.lock().clone()),
            
            tasks: SpinLock::new(Vec::new()),
            cwd: SpinLock::new(parent.cwd.lock().clone()),
            umask: SpinLock::new(*parent.umask.lock()),
            waiting_task: SpinLock::new(Vec::new()),

            signal: Signal {
                actions: Mutex::new(SignalActionTable::new()),
                pending: Mutex::new(PendingSignalQueue::new()),
            },

            children: Mutex::new(Vec::new()),

            itimer_ids: SpinLock::new([None; 3]),
        })
    }

    pub fn new_initprocess(initpath: &str, cwd: &str, argv: &[&str], envp: &[&str]) -> Result<Arc<Self>, Errno> {
        let new_tid = tid::alloc();

        let cwd = vfs::load_dentry(cwd)?;

        let pcb = Arc::new(Self {
            pid: new_tid,
            parent: SpinLock::new(None),
            state: SpinLock::new(State::Running),
            exec_path: SpinLock::new(String::from(initpath)),
            
            tasks: SpinLock::new(Vec::new()),
            cwd: SpinLock::new(cwd.clone()),
            umask: SpinLock::new(0o022),
            waiting_task: SpinLock::new(Vec::new()),

            signal: Signal {
                actions: Mutex::new(SignalActionTable::new()),
                pending: Mutex::new(PendingSignalQueue::new()),
            },

            children: Mutex::new(Vec::new()),

            itimer_ids: SpinLock::new([None; 3]),
        });

        let file = vfs::open_file(
            initpath, 
            FileFlags { readable: true, writable: false, blocked: true },
            &Perm::new(PermFlags::X)
        ).expect("Failed to open init file");

        let first_task = TCB::new_inittask(new_tid, &pcb, file, argv, envp);
        pcb.tasks.lock().push(first_task.clone());

        scheduler::push_task(first_task);

        Ok(pcb)
    }

    pub fn get_pid(&self) -> Tid {
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
            _ => None,
        };
        *state = State::Dead;
        code
    }

    pub fn has_child(&self, tid: Tid) -> bool {
        self.tasks.lock().iter().any(|tcb| tcb.get_tid() == tid)
    }

    pub fn with_cwd<F, R>(&self, f: F) -> R 
    where F: FnOnce(&Arc<Dentry>) -> R {
        let cwd = self.cwd.lock();
        f(&cwd)
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
            manager::insert(new_parent);
        }

        Ok(new_tcb)
    }

    pub fn exec(
        self: &Arc<Self>, 
        tcb: &TCB, 
        file: File,
        exec_path: String, 
        argv: &[&str], 
        envp: &[&str]
    ) -> Result<(), Errno> {        
        let first_task = tcb.new_exec(file, argv, envp)?;

        let mut tasks = self.tasks.lock();
        tasks.iter_mut().for_each(|tcb| {
            tcb.with_state_mut(|state| state.state = TaskState::Exited );
        });
        tasks.clear();
        tasks.push(first_task.clone());

        *self.exec_path.lock() = exec_path;

        scheduler::push_task(first_task);

        Ok(())
    }

    pub fn exit(self: &Arc<Self>, code: u8) {
        let mut task = self.tasks.lock();
        task.iter().for_each(|t| {
            t.with_state_mut(|state| state.state = TaskState::Exited );
        });
        task.clear();

        drop(task);

        *self.state.lock() = State::Exited(code);

        if self.pid == 0 {
            panic!("Init process exited with code {}, system will halt.", code);
        }
        
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

        with_initprocess(|init_process| {
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
            children.iter().find(|c| c.get_pid() == pid).cloned()
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
                children.retain(|c| c.get_pid() != pid);
                
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
            let pid = child.get_pid();
            if let Some(exit_code) = child.recycle() {
                children.retain(|c| c.get_pid() != pid);
                return Ok(Some((pid, exit_code)));
            }
        }

        drop(children);
        
        if !blocked {
            return Ok(None);
        }

        self.waiting_task.lock().push(current::task().clone());

        let event = current::block("wait_any_child");

        match event {
            Event::Process { child } => {
                let pid = child;
                let mut children = self.children.lock();

                let exit_code = match children.iter().find(|c| c.get_pid() == child) {
                    Some(child_pcb) => {
                        if let Some(exit_code) = child_pcb.recycle() {
                            exit_code
                        } else {
                            return Err(Errno::ECHILD);
                        }
                    },
                    None => return Err(Errno::ECHILD), // The child process was recycled by other waiters
                };

                children.retain(|c| c.get_pid() != pid);

                Ok(Some((pid, exit_code)))
            }
            Event::Signal => {
                Err(Errno::EINTR)
            }
            _ => unreachable!(),
        }
    }

    pub fn signal_actions(&self) -> &Mutex<SignalActionTable> {
        &self.signal.actions
    }

    pub fn pending_signals(&self) -> &Mutex<PendingSignalQueue> {
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
