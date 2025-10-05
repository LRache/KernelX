use alloc::vec::Vec;
use alloc::sync::Arc;
use spin::Mutex;

use crate::kernel::errno::{Errno, SysResult};
use crate::kernel::task::def::TaskCloneFlags;
use crate::kernel::task::get_initprocess;
use crate::kernel::task::fdtable::{FDFlags, FDTable};
use crate::kernel::scheduler::{self, current};
use crate::kernel::task::tid::Tid;
use crate::kernel::task::tid;
use crate::kernel::event::Event;
use crate::fs::file::{File, FileFlags};
use crate::fs::vfs;
use crate::fs::Dentry;

use super::tcb::{TCB, TaskState};

pub struct PCB {
    pid: Tid,
    pub parent: Mutex<Option<Arc<PCB>>>,
    is_zombie: Mutex<bool>,
    exit_code: Mutex<u8>,
    
    tasks: Mutex<Vec<Arc<TCB>>>,
    cwd: Mutex<Arc<Dentry>>,
    waiting_task: Mutex<Vec<Arc<TCB>>>,

    children: Mutex<Vec<Arc<PCB>>>,
}

impl PCB {
    pub fn new(pid: i32, parent: &Arc<PCB>, cwd: &Arc<Dentry>) -> Arc<Self> {
        Arc::new(Self {
            pid,
            parent: Mutex::new(Some(parent.clone())),
            is_zombie: Mutex::new(false),
            exit_code: Mutex::new(0),
            
            tasks: Mutex::new(Vec::new()),
            cwd: Mutex::new(cwd.clone()),
            waiting_task: Mutex::new(Vec::new()),

            children: Mutex::new(Vec::new()),
        })
    }

    pub fn new_initprocess(file: File, cwd: &str, argv: &[&str], envp: &[&str]) -> Result<Arc<Self>, Errno> {
        let new_tid = tid::alloc();
        assert!(new_tid == 0);
        
        let file = Arc::new(file);

        let mut fd_table = FDTable::new();
        for _ in 0..3 {
            fd_table.push(vfs::stdout::stdout(), FDFlags::empty())?;
        }
        fd_table.push(file.clone(), FDFlags::empty())?;

        let cwd = vfs::open_dentry(cwd, FileFlags::dontcare())?;

        let pcb = Arc::new(Self {
            pid: 0,
            parent: Mutex::new(None),
            is_zombie: Mutex::new(false),
            exit_code: Mutex::new(0),
            
            tasks: Mutex::new(Vec::new()),
            cwd: Mutex::new(cwd.clone()),
            waiting_task: Mutex::new(Vec::new()),

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
    ) -> Result<Arc<TCB>, Errno> {
        let new_tid = tid::alloc();
        let new_tcb;

        if flags.thread {
            new_tcb = tcb.new_clone(new_tid, self, userstack, flags);
            self.tasks.lock().push(new_tcb.clone());
        } else {
            let new_parent = PCB::new(new_tid, self, &self.cwd.lock());
            new_tcb = tcb.new_clone(new_tid, &new_parent, userstack, flags);
            new_parent.tasks.lock().push(new_tcb.clone());
            self.children.lock().push(new_parent);
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
                t.wakeup_by_event(self.pid as usize, Event::Process);
            });
        }
        
        let mut children = self.children.lock();
        children.iter_mut().for_each(|c| {
            *c.parent.lock() = Some(get_initprocess().clone());
        });

        get_initprocess().children.lock().append(&mut children);
    }

    pub fn exit_task(self: &Arc<Self>, tcb: &Arc<TCB>, code: u8) {
        let mut tasks = self.tasks.lock();
        tasks.retain(|t| !Arc::ptr_eq(t, tcb));

        if tasks.is_empty() {
            self.exit(code);
        }
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
                    
                    if current::tcb().with_state_mut(|state| {
                        match state.event.as_ref() {
                            None => return false,
                            Some(event) => {
                                if event.event == Event::Process && event.waker == pid as usize {
                                    state.event = None;
                                } else {
                                    return false;
                                }
                            }
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

        loop {
            self.waiting_task.lock().push(current::tcb().clone());
            current::tcb().block("wait_any_child");
            // kinfo!("Waiting for child process of PID {} to exit", self.pid);
            current::schedule();

            let mut pid = 0;
            let mut exit_code = 0;
            if current::tcb().with_state_mut(|state| {
                match state.event.as_ref() {
                    None => return false,
                    Some(event) => {
                        if event.event == Event::Process {
                            let mut children = self.children.lock();
                            
                            let child = children.iter().find(|c| c.get_pid() == event.waker as i32);
                            match child {
                                Some(c) => {
                                    pid = c.get_pid();
                                    exit_code = c.get_exit_code();
                                },
                                None => {
                                    return false;
                                }
                            }
                            children.retain(|c| c.get_pid() != event.waker as i32);
                        } else {
                            return false;
                        }
                    }
                };
                state.event = None;
                return true;
            }) {
                return Ok(Some((pid, exit_code)))
            }
        }
    }
}

unsafe impl Send for PCB {}
unsafe impl Sync for PCB {}
