use alloc::sync::Arc;

use crate::arch::UserContextTrait;
use crate::kernel::{config, scheduler};
use crate::kernel::event::Event;
use crate::kernel::ipc::signal::frame::SigFrame;
use crate::kernel::ipc::{KSiFields, SiCode, SignalSet};
use crate::kernel::mm::vdso;
use crate::kernel::task::{PCB, TCB};
use crate::kernel::scheduler::{Tid, current};
use crate::kernel::errno::{SysResult, Errno};

use super::{SignalNum, PendingSignal, SignalDefaultAction, SignalActionFlags};

impl TCB {    
    pub fn handle_signal(&self) {
        let mut state = self.state().lock();
        let signal = match state.pending_signal.take() {
            Some(sig) => sig,
            None => return,
        };
        drop(state);

        let signum = signal.signum;
        if signum.is_empty() {
            return;
        }
        
        if signum.is_kill() {
            self.parent.exit(128 + signum.num() as u8);
            current::schedule();

            unreachable!();
        }
        
        let (action, stack) = {
            let signal_actions = self.parent.signal_actions().lock();
            (signal_actions.get(signal.signum), signal_actions.get_stack_top())
        };
        
        if action.is_default() {
            match signum.default_action() {
                SignalDefaultAction::Term | SignalDefaultAction::Stop | SignalDefaultAction::Core => {
                    self.parent.exit(128 + signum.num() as u8);
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
        sigframe.info.si_code = signal.si_code;
        sigframe.info.fields = signal.fields.into();
        sigframe.info.si_errno = 0;
        sigframe.ucontext.uc_sigmask = old_mask;
        sigframe.ucontext.uc_mcontext = (*self.user_context()).into();
        
        let mut stack_top = if action.flags.contains(SignalActionFlags::SA_ONSTACK) {
            match stack {
                Some(stack_top) => stack_top,
                None => self.user_context().get_user_stack_top(),
            }
        } else {
            self.user_context().get_user_stack_top()
        };
        stack_top -= core::mem::size_of::<SigFrame>();
        stack_top &= !0xf; // Align to 16 bytes
        self.get_addrspace().copy_to_user(stack_top, sigframe).expect("Failed to copy sigframe to user stack");
        
        self.user_context()
            .set_sigaction_restorer(vdso::addr_of("sigreturn_trampoline") + config::VDSO_BASE)
            .set_arg(0, signum.into())
            .set_user_entry(action.handler)
            .set_user_stack_top(stack_top);

        if action.flags.contains(SignalActionFlags::SA_SIGINFO) {
            let siginfo_uaddr  = stack_top + core::mem::offset_of!(SigFrame, info);
            let ucontext_uaddr = stack_top + core::mem::offset_of!(SigFrame, ucontext);
            self.user_context()
                .set_arg(1, siginfo_uaddr)
                .set_arg(2, ucontext_uaddr);
        }
    }

    pub fn return_from_signal(&self) {
        let sigframe = self.get_addrspace()
                           .copy_from_user::<SigFrame>(self.user_context().get_user_stack_top())
                           .expect("Failed to copy sigframe from user stack");
        self.set_signal_mask(sigframe.ucontext.uc_sigmask);
        self.user_context().restore_from_signal(&sigframe.ucontext.uc_mcontext);
    }  

    pub fn try_recive_pending_signal(self: &Arc<Self>, pending: PendingSignal) -> bool {
        let signum = pending.signum;
        if signum.is_empty() {
            return true;
        }
        
        let mut state = self.state().lock();

        let waiting = state.signal_to_wait;
        if waiting.contains(signum) {
            state.pending_signal = Some(pending);
            state.signal_to_wait = SignalSet::empty();
            drop(state);

            scheduler::wakeup_task(self.clone(), Event::WaitSignal { signum });

            return true;
        }
        
        if state.pending_signal.is_some() {
            return false;
        }
        
        if signum.is_unignorable() {
            state.pending_signal = Some(pending);
            drop(state);
            
            scheduler::wakeup_task(self.clone(), Event::Signal);

            return true;
        }

        let mask = self.get_signal_mask();
        if !signum.is_masked(mask) {
            state.pending_signal = Some(pending);
            drop(state);
            
            scheduler::wakeup_task(self.clone(), Event::Signal);
            
            return true;
        } else {
            return false;
        }
    }

    pub fn recive_pending_signal_from_parent(&self) {
        let mut state = self.state().lock();
        
        if state.pending_signal.is_some() {
            drop(state);
            return;
        }

        if let Some(signal) = self.parent.pending_signals().lock().pop_pending(*self.signal_mask.lock(), self.tid) {
            state.pending_signal = Some(signal);
        }
    }
}

impl PCB {
    pub fn send_signal(&self, signum: SignalNum, si_code: SiCode, fields: KSiFields, dest: Option<Tid>) -> SysResult<()> {
        let pending = PendingSignal {
            signum,
            si_code,
            fields,
            dest,
        };

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

        self.pending_signals().lock().add_pending(pending)?;

        Ok(())
    }
}
