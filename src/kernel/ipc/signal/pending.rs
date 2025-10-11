use alloc::vec::Vec;

use crate::kernel::ipc::SignalNum;
use crate::kernel::task::Tid;
use crate::kernel::errno::SysResult;

#[derive(Clone, Copy)]
pub struct PendingSignal {
    pub signum: u32,
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
        assert!(pending.signum < 32);
        self.pending.push(pending);
        Ok(())
    }

    pub fn pop_pending(&mut self, mask: usize) -> Option<PendingSignal> {
        let mut index = None;
        for (i, signal) in self.pending.iter().enumerate() {
            if (mask & (1 << (signal.signum - 1))) != 0 || SignalNum::is_unignorable(signal.signum) {
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
