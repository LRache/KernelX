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
        const SA_INTERRUPT = 0x20000000;
        const SA_NODEFER   = 0x40000000;
        const SA_RESETHAND = 0x80000000;
    }
}

bitflags! {
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub struct SignalStackFlags: usize {
        const SS_ONSTACK = 1 << 0;
        const SS_DISABLE = 1 << 1;
    }
}

#[derive(Clone, Copy, Debug)]
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

#[derive(Clone, Copy, Debug, Default)]
pub struct SignalStackState {
    pub stack: Option<(usize, usize)>, // (ss_sp, ss_size)
    pub on_stack: bool,
}

impl SignalStackState {
    pub fn get_stack_top(&self) -> Option<usize> {
        self.stack.map(|(sp, size)| (sp + size) & !0xf)
    }
}

#[derive(Clone)]
pub struct SignalActionTable {
    actions: [SignalAction; 64],
}

impl SignalActionTable {
    pub fn new() -> Self {
        SignalActionTable {
            actions: [SignalAction::empty(); 64],
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

    pub fn reset_for_exec(&mut self) {
        for action in self.actions.iter_mut() {
            // POSIX: dispositions set to a handler are reset to default on exec;
            // dispositions set to SIG_IGN remain ignored.
            if !action.is_default() && !action.is_ignore() {
                action.handler = SIG_DFL;
                action.mask = SignalSet::empty();
                action.flags = SignalActionFlags::empty();
            }
        }
    }
}
