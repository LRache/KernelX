use alloc::collections::BTreeMap;
use alloc::sync::Arc;
use alloc::vec::Vec;

use crate::kernel::scheduler::Tid;
use crate::kernel::scheduler;
use crate::kernel::task::TCB;
use crate::klib::SpinLock;
use crate::kinfo;

use super::PCB;

pub const INIT_UTASK_TID: Tid = 1;

fn split_with_quotes(input: &str) -> Vec<&str> {
    let mut result = Vec::new();
    let mut in_quotes = false;
    let mut start = None;

    for (i, c) in input.char_indices() {
        match c {
            '"' => {
                in_quotes = !in_quotes;
                if start.is_none() {
                    start = Some(i);
                }
            }
            ' ' if !in_quotes => {
                if let Some(begin) = start {
                    result.push(&input[begin..i]);
                    start = None;
                }
            }
            _ => {
                if start.is_none() {
                    start = Some(i);
                }
            }
        }
    }

    if let Some(begin) = start {
        result.push(&input[begin..]);
    }

    result
        .into_iter()
        .map(|s| s.trim_matches('"'))
        .filter(|s| !s.is_empty())
        .collect()
}

static PCBS: SpinLock<BTreeMap<Tid, Arc<PCB>>> = SpinLock::new(BTreeMap::new());

pub fn create_initprocess(initpath: &str, initcwd: &str, initargs: &str, tty: &str) -> Arc<TCB>{
    let initargv = split_with_quotes(initargs);
    let initenvp: &[&str] = &[];

    kinfo!("Creating init process: \npath='{}', \ncwd='{}', \nargv={:?}, \nenvp={:?}", initpath, initcwd, initargv, initenvp);
        
    let (pcb, tcb) = PCB::new_initprocess(initpath, initcwd, initargv.as_slice(), initenvp, tty).expect("Failed to initialize init process from ELF");

    debug_assert!(pcb.pid() == INIT_UTASK_TID, "Init process must have PID {}, got {}", INIT_UTASK_TID, pcb.pid());
        
    PCBS.lock().insert(pcb.pid(), pcb.clone());
    scheduler::push_task(tcb.clone());
    tcb
}

pub fn with_initpcb<F, R>(f: F) -> R
where
    F: FnOnce(&Arc<PCB>) -> R,
{
    let pcbs = PCBS.lock();
    let pcb = pcbs.get(&INIT_UTASK_TID).expect("Init process not created yet");
    f(pcb)
}

pub fn insert(pcb: Arc<PCB>) {
    PCBS.lock().insert(pcb.pid(), pcb);
}

pub fn get(tid: Tid) -> Option<Arc<PCB>> {
    PCBS.lock().get(&tid).cloned()
}

pub fn remove(tid: Tid) -> Option<Arc<PCB>> {
    PCBS.lock().remove(&tid)
}

pub fn pcbs() -> &'static SpinLock<BTreeMap<Tid, Arc<PCB>>> {
    &PCBS
}
