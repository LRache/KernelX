use core::cell::UnsafeCell;
use core::mem::MaybeUninit;
use alloc::collections::BTreeMap;
use alloc::sync::Arc;
use spin::Mutex;

use crate::fs::file::FileFlags;
use crate::fs::vfs;

use super::Pid;
use super::PCB;

pub struct Manager {
    pcbs: Mutex<BTreeMap<Pid, Arc<PCB>>>,
    initprocess: UnsafeCell<MaybeUninit<Arc<PCB>>>,
}

impl Manager {
    pub const fn new() -> Self {
        Self {
            pcbs: Mutex::new(BTreeMap::new()),
            initprocess: UnsafeCell::new(MaybeUninit::uninit()),
        }
    }

    pub fn create_initprocess(&self, initpath: &str, initcwd: &str) {
        let initargv: &[&str] = &[
            initpath, 
            "sh", 
            "/glibc/libctest_testcode.sh",
        ];

        let initenvp: &[&str] = &[];

        let initfile = vfs::open_file(initpath, FileFlags { readable: true, writable: false }).expect("Failed to open init file");
        let pcb = PCB::new_initprocess(initfile, initcwd, initargv, initenvp).expect("Failed to initialize init process from ELF");
        
        self.pcbs.lock().insert(0, pcb.clone());

        unsafe {
            *self.initprocess.get() = MaybeUninit::new(pcb);
        }
    }

    pub fn get_initprocess(&self) -> &Arc<PCB> {
        unsafe {
            (&*self.initprocess.get()).assume_init_ref()
        }
    }

    pub fn insert_pcb(&self, pcb: Arc<PCB>) {
        let mut pcbs = self.pcbs.lock();
        pcbs.insert(pcb.get_pid(), pcb);
    }

    pub fn get_pcb(&self, pid: Pid) -> Option<Arc<PCB>> {
        let pcbs = self.pcbs.lock();
        pcbs.get(&pid).cloned()
    }

    pub fn remove_pcb(&self, pid: Pid) -> Option<Arc<PCB>> {
        let mut pcbs = self.pcbs.lock();
        pcbs.remove(&pid)
    }
}

unsafe impl Sync for Manager {}

static MANAGER: Manager = Manager::new();

pub fn create_initprocess(initpath: &str, initcwd: &str) {
    MANAGER.create_initprocess(initpath, initcwd);
}

pub fn get_initprocess() -> &'static Arc<PCB> {
    MANAGER.get_initprocess()
}

pub fn insert(pcb: Arc<PCB>) {
    MANAGER.insert_pcb(pcb);
}

pub fn get(pid: Pid) -> Option<Arc<PCB>> {
    MANAGER.get_pcb(pid)
}

pub fn remove(pid: Pid) -> Option<Arc<PCB>> {
    MANAGER.remove_pcb(pid)
}
