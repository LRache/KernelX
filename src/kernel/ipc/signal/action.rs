use crate::kernel::errno::{Errno, SysResult};

use super::SignalSet;

const SIG_DFL: usize = 0;
const SIG_IGN: usize = 1;

#[derive(Clone, Copy)]
pub struct SignalAction {
    pub handler: usize,
    pub restorer: usize,
    pub mask: SignalSet
}

impl SignalAction {
    pub const fn empty() -> Self {
        SignalAction {
            handler: 0,
            restorer: 0,
            mask: 0,
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

    pub fn get(&self, signum: u32) -> SysResult<SignalAction> {
        if signum == 0 || signum > 32 {
            return Err(Errno::EINVAL);
        }
        Ok(self.actions[signum as usize - 1])
    }

    pub fn set(&mut self, signum: u32, action: &SignalAction) -> SysResult<()> {
        if signum == 0 || signum > 32 {
            return Err(Errno::EINVAL);
        }
        self.actions[signum as usize - 1] = *action;
        Ok(())
    }
}
