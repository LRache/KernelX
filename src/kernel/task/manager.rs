use alloc::collections::BTreeMap;
use alloc::sync::Arc;
use alloc::vec::Vec;

use crate::kernel::scheduler::{Tid, tid};
use crate::kernel::scheduler;
use crate::kernel::task::TCB;
use crate::kinfo;
use crate::klib::SpinLock;

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

static TCBS: SpinLock<BTreeMap<Tid, Arc<TCB>>> = SpinLock::new(BTreeMap::new());

pub fn create_initprocess(initpath: &str, initcwd: &str, initargs: &str) {
    let initargv = split_with_quotes(initargs);
    let initenvp: &[&str] = &[];

    kinfo!("Creating init process: \npath='{}', \ncwd='{}', \nargv={:?}, \nenvp={:?}", initpath, initcwd, initargv, initenvp);
        
    let tcb = PCB::new_initprocess(initpath, initcwd, initargv.as_slice(), initenvp).expect("Failed to initialize init process from ELF");

    debug_assert!(tcb.tid() == tid::TID_START, "Init process must have PID 1, got {}", tcb.tid());
        
    TCBS.lock().insert(tid::TID_START, tcb.clone());
    scheduler::push_task(tcb);
}

pub fn with_initpcb<F, R>(f: F) -> R
where
    F: FnOnce(&Arc<PCB>) -> R,
{
    let tcbs = TCBS.lock();
    let initprocess = tcbs.get(&tid::TID_START).expect("Init process not created yet");
    f(initprocess.parent())
}

pub fn insert(tcb: Arc<TCB>) {
    TCBS.lock().insert(tcb.tid(), tcb);
}

pub fn get(tid: Tid) -> Option<Arc<TCB>> {
    TCBS.lock().get(&tid).cloned()
}

pub fn remove(tid: Tid) -> Option<Arc<TCB>> {
    TCBS.lock().remove(&tid)
}

pub fn tcbs() -> &'static SpinLock<BTreeMap<Tid, Arc<TCB>>> {
    &TCBS
}
