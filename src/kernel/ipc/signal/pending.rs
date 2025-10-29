use alloc::vec::Vec;

use crate::kernel::ipc::{SignalNum, SignalSet};
use crate::kernel::task::Tid;
use crate::kernel::errno::SysResult;

#[derive(Clone, Copy, Debug)]
pub struct PendingSignal {
    pub signum: SignalNum,
    pub sender: Tid,
    pub dest: Option<Tid>,
}

pub struct PendingSignalQueue {
    pending: Vec<PendingSignal>
}

impl PendingSignalQueue {
    pub fn new() -> Self {
        PendingSignalQueue {
            pending: Vec::new(),
        }
    }

    pub fn add_pending(&mut self, pending: PendingSignal) -> SysResult<()> {
        self.pending.push(pending);
        Ok(())
    }

    pub fn pop_pending(&mut self, mask: SignalSet) -> Option<PendingSignal> {
        let mut index = None;
        for (i, signal) in self.pending.iter().enumerate() {
            if !signal.signum.is_masked(mask) || signal.signum.is_unignorable() {
                index = Some(i);
                break;
            }
        }
        if let Some(i) = index {
            Some(self.pending.remove(i))
        } else {
            None
        }
    }
}
