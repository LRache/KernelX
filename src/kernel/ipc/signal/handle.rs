use alloc::sync::Arc;

use crate::arch::UserContextTrait;
use crate::kernel::config;
use crate::kernel::event::Event;
use crate::kernel::ipc::signal::frame::SigFrame;
use crate::kernel::ipc::SignalSet;
use crate::kernel::mm::vdso;
use crate::kernel::task::{Tid, PCB, TCB};
use crate::kernel::scheduler::current;
use crate::kernel::errno::{SysResult, Errno};
use crate::lock_debug;

use super::{SignalNum, PendingSignal, SignalDefaultAction, SignalActionFlags};

impl TCB {    
    pub fn handle_signal(&self) {
        let mut state = self.state().lock();
        let signal = match state.pending_signal.take() {
            Some(sig) => sig,
            None => return,
        };
        drop(state);
        // kinfo!("signal_pending before handle_signal: {:?}", signal_pending);

        let signum = signal.signum;
        if signum.is_empty() {
            return;
        }
        
        if signum.is_kill() {
            self.parent.exit(0);
            current::schedule();

            unreachable!();
        }
        
        let action = self.parent.signal_actions().lock().get(signal.signum);
        if action.is_default() {
            match signum.default_action() {
                SignalDefaultAction::Term | SignalDefaultAction::Stop | SignalDefaultAction::Core => {
                    self.parent.exit(255);
                    current::schedule();

                    unreachable!();
                },
                _ => return
            }
        } else if action.is_ignore() {
            return;
        }

        let old_mask = self.get_signal_mask();
        let mut new_mask = old_mask | action.mask;
        if !action.flags.contains(SignalActionFlags::SA_NODEFER) {
            new_mask |= signum.to_mask_set();
        }
        self.set_signal_mask(new_mask);

        let mut sigframe = SigFrame::empty();
        sigframe.info.si_signo = Into::<u32>::into(signum) as i32;
        sigframe.info.si_code = 0;
        sigframe.info.si_errno = 0;
        sigframe.ucontext.uc_sigmask = old_mask;
        sigframe.ucontext.uc_mcontext = (*self.user_context()).into();
        
        self.user_context()
            .set_sigaction_restorer(vdso::addr_of("sigreturn_trampoline") + config::VDSO_BASE)
            .set_user_entry(action.handler);
        
        let mut stack_top = self.user_context().get_user_stack_top();
        stack_top -= core::mem::size_of::<SigFrame>();
        stack_top &= !0xf; // Align to 16 bytes
        self.get_addrspace().copy_to_user_object(stack_top, sigframe).expect("Failed to copy sigframe to user stack");
        self.user_context().set_user_stack_top(stack_top);

        // kinfo!("handle signal for task {}, signum={:?}, action.handler={:#x}, stack_top={:#x}, pending={:?}", self.tid, signum, action.handler, stack_top, signal_pending);
    }

    pub fn return_from_signal(&self) {
        let sigframe = self.get_addrspace().copy_from_user::<SigFrame>(self.user_context().get_user_stack_top())
            .expect("Failed to copy sigframe from user stack");
        // kinfo!("return from signal for task {}, ucontext={:?}", self.tid, sigframe.ucontext.uc_mcontext);
        self.user_context().restore_from_signal(&sigframe.ucontext.uc_mcontext);
    }  

    pub fn try_recive_pending_signal(self: &Arc<Self>, pending: PendingSignal) -> bool {
        let mut state = lock_debug!(self.state());

        // if state.state != TaskState::Blocked {
        //     return false;
        // }
        
        if state.pending_signal.is_some() {
            return false;
        }

        // kinfo!("try_recive_pending_signal: task {} checking pending signal {:?}", self.tid, pending);
        
        let signum = pending.signum;
        if signum.is_unignorable() {
            state.pending_signal = Some(pending);
            drop(state);
            
            self.wakeup(Event::Signal);

            return true;
        }

        let waiting = state.waiting_signal;
        if waiting.contains(signum) {
            state.pending_signal = Some(pending);
            state.waiting_signal = SignalSet::empty();
            drop(state);

            self.wakeup(Event::WaitSignal { signum });

            return true;
        }

        let mask = self.get_signal_mask();
        if !signum.is_masked(mask) {
            state.pending_signal = Some(pending);
            drop(state);
            
            self.wakeup(Event::Signal);
            
            return true;
        } else {
            return false;
        }
    }
}

impl PCB {
    pub fn send_signal(&self, signum: SignalNum, sender: Tid, dest: Option<Tid>) -> SysResult<()> {
        let pending = PendingSignal {
            signum,
            sender,
            dest,
        };

        let action = self.signal_actions().lock().get(signum);
        
        if action.is_ignore() && !signum.is_unignorable() {
            return Ok(());
        }

        if action.is_default() && signum.default_action() == SignalDefaultAction::Ign {
            return Ok(());
        }

        if let Some(dest) = dest {
            let tasks = self.tasks.lock();
            if let Some(task) = tasks.iter().find(|t| t.tid == dest).cloned() {
                if !task.try_recive_pending_signal(pending) {
                    self.pending_signals().lock().add_pending(pending)?;
                }
                return Ok(());
            } else {
                return Err(Errno::ESRCH);
            }
        }

        for task in self.tasks.lock().iter() {
            if task.try_recive_pending_signal(pending) {
                return Ok(())
            }
        }

        // self.pending_signals().lock().add_pending(pending)?;

        Ok(())
    }
}
