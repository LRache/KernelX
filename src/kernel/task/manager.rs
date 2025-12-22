use alloc::collections::BTreeMap;
use alloc::sync::Arc;
use alloc::vec::Vec;

use crate::kernel::scheduler::{Tid, tid};
use crate::kernel::scheduler;
use crate::klib::SpinLock;
use crate::kinfo;

use super::PCB;

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

pub fn create_initprocess(initpath: &str, initcwd: &str, initargs: &str) {
    let initargv = split_with_quotes(initargs);
    let initenvp: &[&str] = &[];

    kinfo!("Creating init process: \npath='{}', \ncwd='{}', \nargv={:?}, \nenvp={:?}", initpath, initcwd, initargv, initenvp);
        
    let pcb = PCB::new_initprocess(initpath, initcwd, initargv.as_slice(), initenvp).expect("Failed to initialize init process from ELF");

    debug_assert!(pcb.pid() == tid::TID_START, "Init process must have PID 1, got {}", pcb.pid());
        
    PCBS.lock().insert(tid::TID_START, pcb.clone());
    scheduler::push_task(pcb.tasks.lock()[0].clone());
}

pub fn with_initpcb<F, R>(f: F) -> R
where
    F: FnOnce(&Arc<PCB>) -> R,
{
    let pcbs = PCBS.lock();
    let pcb = pcbs.get(&tid::TID_START).expect("Init process not created yet");
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
