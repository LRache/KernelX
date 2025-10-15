use bitflags::bitflags;

use crate::kernel::errno::{Errno, SysResult};

use super::SignalSet;

const SIG_DFL: usize = 0;
const SIG_IGN: usize = 1;

bitflags! {
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub struct SignalActionFlags: u32 {
        const SA_NOCLDSTOP = 0x0001;
        const SA_NOCLDWAIT = 0x0002;
        const SA_SIGINFO   = 0x0004;
        const SA_ONSTACK   = 0x0008;
        const SA_RESTART   = 0x0010;
        const SA_NODEFER   = 0x0020;
        const SA_RESETHAND = 0x0040;
    }
}

#[derive(Clone, Copy)]
pub struct SignalAction {
    pub handler: usize,
    // pub restorer: usize,
    pub mask: SignalSet,
    pub flags: SignalActionFlags,
}

impl SignalAction {
    pub const fn empty() -> Self {
        SignalAction {
            handler: 0,
            // restorer: 0,
            mask: 0,
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
    pub actions: [SignalAction; 32],
}

impl SignalActionTable {
    pub fn new() -> Self {
        SignalActionTable {
            actions: [SignalAction::empty(); 32],
        }
    }

    pub fn get(&self, signum: u32) -> SignalAction {
        assert!(signum > 0 && signum <= 32);
        self.actions[signum as usize - 1]
    }

    pub fn set(&mut self, signum: u32, action: &SignalAction) -> SysResult<()> {
        if signum == 0 || signum > 32 {
            return Err(Errno::EINVAL);
        }
        self.actions[signum as usize - 1] = *action;
        Ok(())
    }
}
