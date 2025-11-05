use alloc::vec::Vec;
use alloc::sync::Arc;
use spin::Mutex;

use crate::kernel::errno::{Errno, SysResult};
use crate::kernel::task::def::TaskCloneFlags;
use crate::kernel::task::{get_initprocess, manager};
use crate::kernel::scheduler::{self, current};
use crate::kernel::task::tid::Tid;
use crate::kernel::task::tid;
use crate::kernel::event::Event;
use crate::kernel::ipc::{SignalActionTable, PendingSignalQueue, signum};
use crate::fs::file::File;
use crate::fs::vfs;
use crate::fs::Dentry;
use crate::klib::SpinLock;
use crate::lock_debug;

use super::tcb::{TCB, TaskState};

struct Signal {
    actions: Mutex<SignalActionTable>,
    pending: Mutex<PendingSignalQueue>,
}

pub struct PCB {
    pid: Tid,
    pub parent: SpinLock<Option<Arc<PCB>>>,
    is_zombie: SpinLock<bool>,
    exit_code: SpinLock<u8>,
    
    pub tasks: SpinLock<Vec<Arc<TCB>>>,
    cwd: SpinLock<Arc<Dentry>>,
    waiting_task: SpinLock<Vec<Arc<TCB>>>,

    signal: Signal,

    children: Mutex<Vec<Arc<PCB>>>,
}

impl PCB {
    pub fn new(pid: i32, parent: &Arc<PCB>, cwd: &Arc<Dentry>) -> Arc<Self> {
        Arc::new(Self {
            pid,
            parent: SpinLock::new(Some(parent.clone())),
            is_zombie: SpinLock::new(false),
            exit_code: SpinLock::new(0),
            
            tasks: SpinLock::new(Vec::new()),
            cwd: SpinLock::new(cwd.clone()),
            waiting_task: SpinLock::new(Vec::new()),

            signal: Signal {
                actions: Mutex::new(SignalActionTable::new()),
                pending: Mutex::new(PendingSignalQueue::new()),
            },

            children: Mutex::new(Vec::new()),
        })
    }

    pub fn new_initprocess(file: File, cwd: &str, argv: &[&str], envp: &[&str]) -> Result<Arc<Self>, Errno> {
        let new_tid = tid::alloc();
        // assert!(new_tid == 1);
        
        let file = Arc::new(file);

        let cwd = vfs::load_dentry(cwd)?;

        let pcb = Arc::new(Self {
            pid: 0,
            parent: SpinLock::new(None),
            is_zombie: SpinLock::new(false),
            exit_code: SpinLock::new(0),
            
            tasks: SpinLock::new(Vec::new()),
            cwd: SpinLock::new(cwd.clone()),
            waiting_task: SpinLock::new(Vec::new()),

            signal: Signal {
                actions: Mutex::new(SignalActionTable::new()),
                pending: Mutex::new(PendingSignalQueue::new()),
            },

            children: Mutex::new(Vec::new()),
        });

        let first_task = TCB::new_inittask(new_tid, &pcb, &file, argv, envp);
        pcb.tasks.lock().push(first_task.clone());

        scheduler::push_task(first_task);

        Ok(pcb)
    }

    pub fn get_pid(&self) -> Tid {
        self.pid
    }

    fn is_zombie(&self) -> bool {
        *self.is_zombie.lock()
    }

    fn get_exit_code(&self) -> u8 {
        *self.exit_code.lock()
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

    pub fn clone_task(
        self: &Arc<Self>, 
        tcb: &Arc<TCB>, 
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
            let new_parent = PCB::new(new_tid, self, &self.cwd.lock());
            new_tcb = tcb.new_clone(new_tid, &new_parent, userstack, flags, tls);
            new_parent.tasks.lock().push(new_tcb.clone());
            self.children.lock().push(new_parent.clone());
            manager::insert(new_parent);
        }
        
        scheduler::push_task(new_tcb.clone());

        Ok(new_tcb)
    }

    pub fn exec(self: &Arc<Self>, tcb: &Arc<TCB>, file: File, argv: &[&str], envp: &[&str]) -> Result<(), Errno> {        
        let first_task = tcb.new_exec(file, argv, envp)?;

        self.tasks.lock().iter_mut().for_each(|tcb| {
            tcb.with_state_mut(|state| state.state = TaskState::Exited );
        });
        self.tasks.lock().clear();
        self.tasks.lock().push(first_task.clone());

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

        *self.is_zombie.lock() = true;
        *self.exit_code.lock() = code;

        if self.pid == 0 {
            panic!("Init process exited with code {}, system will halt.", code);
        }
        
        if let Some(parent) = self.parent.lock().as_ref() {
            parent.waiting_task.lock().drain(..).for_each(|t| {
                t.wakeup(Event::Process { child: self.pid });
            });
            parent.send_signal(signum::SIGCHLD, current::tid(), None).unwrap_or(());
        }
        
        let mut children = self.children.lock();
        children.iter_mut().for_each(|c| {
            *c.parent.lock() = Some(get_initprocess().clone());
        });

        manager::remove(self.pid);

        get_initprocess().children.lock().append(&mut children);
    }

    pub fn wait_child(&self, pid: i32, blocked: bool) -> Result<Option<u8>, Errno> {
        let child = {
            let children = self.children.lock();
            children.iter().find(|c| c.get_pid() == pid).cloned()
        };
        
        if let Some(child) = child {
            if child.is_zombie() {
                let exit_code = child.get_exit_code();
                // self.children.lock().retain(|c| c.get_pid() != pid);
                return Ok(Some(exit_code));
            }
            
            if blocked {
                loop {
                    self.waiting_task.lock().push(current::tcb().clone());
                    
                    current::tcb().block("wait_child");
                    current::schedule();

                    let state = current::tcb().state().lock();
                    match state.event {
                        Some(Event::Process { child }) => {
                            if child == pid {
                                break;
                            }
                        }

                        Some(Event::Signal) => {
                            return Err(Errno::EINTR);
                        }

                        _ => unreachable!(),
                    }
                    
                    if current::tcb().with_state_mut(|state| {
                        match state.event {
                            Some(Event::Process { child }) => {
                                if child == pid {
                                    state.event = None;
                                } else {
                                    return false;
                                }
                            },
                            _ => return false,
                        };
                        state.event = None;
                        return true;
                    }) {
                        break;
                    }
                }
                
                let exit_code = child.get_exit_code();
                {
                    let mut children = self.children.lock();
                    children.retain(|c| c.get_pid() != pid);
                }
                
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
    
        if let Some(child) = children.iter().find(|c| c.is_zombie()) {
            let pid = child.get_pid();
            let exit_code = child.get_exit_code();
            children.retain(|c| c.get_pid() != pid);
            return Ok(Some((pid, exit_code)));
        }

        drop(children);
        
        if !blocked {
            return Ok(None);
        }

        self.waiting_task.lock().push(current::tcb().clone());
        current::tcb().block("wait_any_child");
        current::schedule();

        let state = lock_debug!(current::tcb().state());

        match state.event {
            Some(Event::Process { child }) => {
                let pid = child;
                let exit_code;
                let mut children = self.children.lock();

                match children.iter().find(|c| c.get_pid() == child){
                    Some(child_pcb) => {
                        exit_code = child_pcb.get_exit_code();
                    },
                    None => unreachable!(), // The child must exist
                }

                children.retain(|c| c.get_pid() != pid);

                Ok(Some((pid, exit_code)))
            }
            Some(Event::Signal) => {
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
}

unsafe impl Send for PCB {}
unsafe impl Sync for PCB {}
