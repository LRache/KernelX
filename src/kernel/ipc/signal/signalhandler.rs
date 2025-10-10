use alloc::{sync::Arc, vec::Vec};

use crate::kernel::task::{Tid, TCB};
use crate::kernel::errno::{Errno, SysResult};
use crate::kernel::scheduler::current;

use super::SignalNum;
use super::SignalSet;

#[derive(Clone, Copy)]
pub struct PendingSignal {
    pub signum: u32,
    pub sender: Tid,
}

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

const SIG_MAX: usize = 31;

pub struct SignalHandler {
    pub actions: [SignalAction; SIG_MAX],
    pub pending: Vec<PendingSignal>
}

impl SignalHandler {
    pub fn new() -> Self {
        SignalHandler {
            actions: [SignalAction::empty(); SIG_MAX],
            pending: Vec::new(),
        }
    }

    pub fn get_sigaction(&self, signum: u32) -> SysResult<SignalAction> {
        if signum >= 32 {
            return Err(Errno::EINVAL);
        }
        Ok(self.actions[signum as usize - 1])
    }

    pub fn set_sigaction(&mut self, signum: u32, action: &SignalAction) -> SysResult<()> {
        if signum >= 32 {
            return Err(Errno::EINVAL);
        }
        self.actions[signum as usize - 1] = *action;
        Ok(())
    }

    pub fn add_pending(&mut self, signum: u32, sender: Tid) -> SysResult<()> {
        assert!(signum < 32);
        self.pending.push(PendingSignal { 
            signum,
            sender 
        });
        Ok(())
    }
}

fn enter_signal(tcb: &Arc<TCB>, signal: &PendingSignal, signal_handler: &SignalHandler) {
    let signum = signal.signum;
    if signum == SignalNum::SIGKILL as u32 || signum == SignalNum::SIGSTOP as u32 {
        tcb.parent.exit(0);
        current::schedule();

        unreachable!();
    }

    let action = signal_handler.get_sigaction(signum).unwrap();

    if action.is_ignore() {
        return;
    }
}

pub fn handle_signal() {
    let signal_handler = current::pcb().signal_handler().lock();
    let tcb = current::tcb();
}
