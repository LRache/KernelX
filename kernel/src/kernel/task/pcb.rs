use alloc::vec::Vec;
use alloc::sync::Arc;
use spin::Mutex;

use crate::kernel::errno::Errno;
use crate::kernel::task::def::TaskCloneFlags;
use crate::kernel::task::get_initprocess;
use crate::kernel::task::fdtable::FDTable;
use crate::kernel::scheduler::{self, current};
use crate::kernel::task::tid::Tid;
use crate::kernel::task::tid;
use crate::fs::file::{File, FileFlags};
use crate::fs::vfs;
use crate::fs::Dentry;
use crate::{kdebug, ktrace};

use super::tcb::{TCB, ThreadState};

pub struct PCB {
    pid: Tid,
    is_zombie: Mutex<bool>,
    exit_code: Mutex<u8>,
    
    tasks: Mutex<Vec<Arc<TCB>>>,
    pwd: Mutex<Arc<Dentry>>,

    children: Mutex<Vec<Arc<PCB>>>,
}

impl PCB {
    pub fn new(pid: i32, pwd: &Arc<Dentry>) -> Arc<Self> {
        Arc::new(Self {
            pid,
            is_zombie: Mutex::new(false),
            exit_code: Mutex::new(0),
            
            tasks: Mutex::new(Vec::new()),
            pwd: Mutex::new(pwd.clone()),

            children: Mutex::new(Vec::new()),
        })
    }

    pub fn new_initprocess(file: File, pwd: &str, argv: &[&str], envp: &[&str]) -> Result<Arc<Self>, Errno> {
        let new_tid = tid::alloc();
        assert!(new_tid == 0);
        
        let file = Arc::new(file);

        let fd_table = FDTable::new();
        for _ in 0..3 {
            fd_table.push(vfs::stdout::stdout())?;
        }
        fd_table.push(file.clone())?;

        let pwd = vfs::open_dentry(pwd, FileFlags::dontcare())?;

        let pcb = PCB::new(new_tid, &pwd);

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

    pub fn with_pwd<F, R>(&self, f: F) -> R 
    where F: FnOnce(&Arc<Dentry>) -> R {
        let pwd = self.pwd.lock();
        f(&pwd)
    }

    pub fn set_pwd(&self, dentry: &Arc<Dentry>) {
        *self.pwd.lock() = dentry.clone();
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
            let new_parent = PCB::new(new_tid, &self.pwd.lock());
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
            tcb.set_state(ThreadState::Exited);
        });
        self.tasks.lock().clear();
        self.tasks.lock().push(first_task.clone());

        scheduler::push_task(first_task);

        Ok(())
    }

    pub fn exit(self: &Arc<Self>, code: u8) {
        for child_task in self.tasks.lock().iter() {
            child_task.set_state(ThreadState::Exited);
        }

        *self.is_zombie.lock() = true;
        *self.exit_code.lock() = code;

        if self.pid == 0 {
            panic!("Init process exited, system will halt.");
        }

        let mut children = self.children.lock();
        while let Some(child) = children.pop() {
            get_initprocess().children.lock().push(child);
        }
    }

    pub fn wait_child(&self, pid: i32, blocked: bool) -> Result<Option<u8>, Errno> {
        let child = {
            let children = self.children.lock();
            children.iter().find(|c| c.get_pid() == pid).cloned()
        };

        kdebug!("Waiting for child with PID: {}", pid);
        
        if let Some(child) = child {
            if blocked {
                while !child.is_zombie() {
                    current::schedule();
                }
                
                let exit_code = child.get_exit_code();
                
                {
                    let mut children = self.children.lock();
                    children.retain(|c| c.get_pid() != pid);
                }
                
                return Ok(Some(exit_code));
            } else {
                if child.is_zombie() {
                    let exit_code = child.get_exit_code();
                    self.children.lock().retain(|c| c.get_pid() != pid);
                    return Ok(Some(exit_code));
                } else {
                    return Ok(None);
                }
            }
        } else { // No child found
            if blocked {
                return Err(Errno::ECHILD);
            } else {
                return Ok(None);
            }
        }
    }

    pub fn wait_any_child(&self, blocked: bool) -> Result<Option<(i32, u8)>, Errno> {
        if blocked {
            loop {
                {
                    let mut children = self.children.lock();
                    if let Some(child) = children.iter().find(|c| c.is_zombie()) {
                        let pid = child.get_pid();
                        let exit_code = child.get_exit_code();
                        children.retain(|c| c.get_pid() != pid);
                        ktrace!("Found child process with PID: {}, exit code: {}", pid, exit_code);
                        return Ok(Some((pid, exit_code)));
                    }
                }
                
                ktrace!("Waiting 0");
                current::schedule();
                ktrace!("Waiting 1");
            }
        } else {
            let children = self.children.lock();
            if let Some(child) = children.iter().find(|c| c.is_zombie()) {
                let pid = child.get_pid();
                let exit_code = child.get_exit_code();
                self.children.lock().retain(|c| c.get_pid() != pid);
                return Ok(Some((pid, exit_code)));
            } else {
                return Ok(None);
            }
        }
    }
}

unsafe impl Send for PCB {}
unsafe impl Sync for PCB {}
