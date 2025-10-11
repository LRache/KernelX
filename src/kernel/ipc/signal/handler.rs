use crate::kernel::task::{TCB, PCB, Tid};
use crate::kernel::scheduler::current;
use crate::kernel::errno::{SysResult, Errno};

use super::{SignalNum, PendingSignal, SignalAction};

impl TCB {
    fn enter_signal(&self, signal: &PendingSignal, action: &SignalAction) {
        let signum = signal.signum;
        if signum == SignalNum::SIGKILL as u32 || signum == SignalNum::SIGSTOP as u32 {
            self.parent.exit(0);
            current::schedule();

            unreachable!();
        }

        if action.is_ignore() {
            return;
        }
    }
    
    pub fn handle_signals(&self) {
        let mut pending_signals = self.parent.pending_signals().lock();
        let mask = self.get_signal_mask();
        let pending = pending_signals.pop_pending(mask);
        drop(pending_signals);

        match pending {
            Some(signal) => {
                let action = self.parent.signal_actions().lock().get(signal.signum).unwrap();
                self.enter_signal(&signal, &action);
            },
            None => {}
        }
    }

    pub fn send_pending_signal(&self, pending: PendingSignal) -> bool {
        let mut task_pending = self.pending_signal.lock();
        if task_pending.is_some() {
            return false;
        }
        
        let signum = pending.signum;
        if SignalNum::is_unignorable(signum) {
            *task_pending = Some(pending);

            return true;
        }

        let mask = *self.signal_mask.lock();
        if !SignalNum::is_masked(signum, mask) {
            *task_pending = Some(pending);
            return true;
        } else {
            return false;
        }
    }
}

impl PCB {
    pub fn send_signal(&self, signum: u32, sender: Tid, dest: Option<Tid>) -> SysResult<()> {
        let pending = PendingSignal {
            signum,
            sender,
            dest,
        };

        if let Some(dest) = dest {
            let tasks = self.tasks.lock();
            if let Some(task) = tasks.iter().find(|t| t.tid == dest).cloned() {
                if !task.send_pending_signal(pending) {
                    self.pending_signals().lock().add_pending(pending)?;
                }
                return Ok(());
            } else {
                return Err(Errno::ESRCH);
            }
        }

        for task in self.tasks.lock().iter() {
            if task.send_pending_signal(pending) {
                return Ok(())
            }
        }

        self.pending_signals().lock().add_pending(pending)?;

        Ok(())
    }
}

pub fn handle_signals() {
    current::tcb().handle_signals();
}
