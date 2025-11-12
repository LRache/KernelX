use bitflags::bitflags;

use crate::kernel::errno::SysResult;
use crate::kernel::ipc::SignalNum;

use super::SignalSet;

const SIG_DFL: usize = 0;
const SIG_IGN: usize = 1;

bitflags! {
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub struct SignalActionFlags: u32 {
        const SA_NOCLDSTOP = 0x1;
        const SA_NOCLDWAIT = 0x2;
        const SA_SIGINFO   = 0x4;
        const SA_RESTORER  = 0x04000000;
        const SA_ONSTACK   = 0x08000000;
        const SA_RESTART   = 0x10000000;
        const SA_NODEFER   = 0x40000000;
        const SA_RESETHAND = 0x80000000;
    }
}

#[derive(Clone, Copy)]
pub struct SignalAction {
    pub handler: usize,
    pub mask: SignalSet,
    pub flags: SignalActionFlags,
}

impl SignalAction {
    pub const fn empty() -> Self {
        SignalAction {
            handler: SIG_DFL,
            mask: SignalSet::empty(),
            flags: SignalActionFlags::empty(),
        }
    }

    pub const fn is_default(&self) -> bool {
        self.handler == SIG_DFL
    }

    pub const fn is_ignore(&self) -> bool {
        self.handler == SIG_IGN
    }
}

pub struct SignalActionTable {
    pub actions: [SignalAction; 63],
}

impl SignalActionTable {
    pub fn new() -> Self {
        SignalActionTable {
            actions: [SignalAction::empty(); 63],
        }
    }

    pub fn get(&self, signum: SignalNum) -> SignalAction {
        let index: usize = signum.into();
        self.actions[index - 1]
    }

    pub fn set(&mut self, signum: SignalNum, action: &SignalAction) -> SysResult<()> {
        let index: usize = signum.into();
        self.actions[index - 1] = *action;
        Ok(())
    }
}
