use alloc::collections::BTreeMap;
use alloc::sync::Arc;

use crate::kernel::scheduler::{Tid, tid};
use crate::klib::SpinLock;

use super::{PCB, Pid};

pub struct Manager {
    pcbs: SpinLock<BTreeMap<Pid, Arc<PCB>>>,
}

impl Manager {
    const fn new() -> Self {
        Self {
            pcbs: SpinLock::new(BTreeMap::new()),
        }
    }

    fn create_initprocess(&self, initpath: &str, initcwd: &str) {
        let initargv: &[&str] = &[
            initpath,
            // "find",
            // "main.c",
            // "-name",
            // "\"busybox_cmd.txt\""
            "sh",
            // "/runall.sh",
            // "lmbench_testcode.sh"
            // "cyclictest_testcode.sh"
        ];

        let initenvp: &[&str] = &[];
        
        let pcb = PCB::new_initprocess(initpath, initcwd, initargv, initenvp).expect("Failed to initialize init process from ELF");

        debug_assert!(pcb.get_pid() == tid::TID_START, "Init process must have PID 1, got {}", pcb.get_pid());
        
        self.pcbs.lock().insert(tid::TID_START, pcb.clone());
    }

    fn with_initprocess<F, R>(&self, f: F) -> R
    where
        F: FnOnce(&Arc<PCB>) -> R,
    {
        let pcbs = self.pcbs.lock();
        let initprocess = pcbs.get(&tid::TID_START).expect("Init process not created yet");
        f(initprocess)
    }

    fn insert_pcb(&self, pcb: Arc<PCB>) {
        let mut pcbs = self.pcbs.lock();
        pcbs.insert(pcb.get_pid(), pcb);
    }

    fn get_pcb(&self, pid: Pid) -> Option<Arc<PCB>> {
        let pcbs = self.pcbs.lock();
        pcbs.get(&pid).cloned()
    }

    fn remove_pcb(&self, pid: Pid) -> Option<Arc<PCB>> {
        let mut pcbs = self.pcbs.lock();
        pcbs.remove(&pid)
    }

    fn find_task_parent(&self, tid: Tid) -> Option<Arc<PCB>> {
        self.pcbs.lock().iter().find(|(_, pcb)| {
            pcb.has_child(tid)
        }).map(|(_, pcb)| pcb.clone())
    }
}

unsafe impl Sync for Manager {}

static MANAGER: Manager = Manager::new();

pub fn create_initprocess(initpath: &str, initcwd: &str) {
    MANAGER.create_initprocess(initpath, initcwd);
}

pub fn with_initprocess<F, R>(f: F) -> R
where
    F: FnOnce(&Arc<PCB>) -> R,
{
    MANAGER.with_initprocess(f)
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

pub fn find_task_parent(tid: Tid) -> Option<Arc<PCB>> {
    MANAGER.find_task_parent(tid)
}
