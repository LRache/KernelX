use alloc::sync::Arc;

use crate::arch::UserContextTrait;
use crate::kernel::errno::{Errno, SysResult};
use crate::kernel::event::Event;
use crate::kernel::ipc::signal::frame::SigFrame;
use crate::kernel::ipc::{signum, KSiFields, SiCode, SignalSet};
use crate::kernel::mm::vdso;
use crate::kernel::scheduler::{Tid, WakeupFailure};
use crate::kernel::task::{ExitStatus, PCB, TCB};
use crate::kernel::{config, scheduler};

use super::frame::SignalStack;
use super::{PendingSignal, SignalActionFlags, SignalDefaultAction, SignalNum};

impl TCB {
    /// Handle a pending signal if there is one. Returns true if a signal was handled and the task's execution context was modified to handle the signal, false otherwise.
    pub fn handle_signal(&self) -> bool {
        let mut state = self.state().lock();
        let signal = match state.pending_signal.take() {
            Some(sig) => sig,
            None => return false,
        };
        drop(state);

        let signum = signal.signum;
        if signum.is_empty() {
            return false;
        }

        if signum.is_kill() {
            self.parent().exit(crate::kernel::task::ExitStatus::Signal {
                sig: signum.num() as u8,
                coredump: false,
            });
            return true;
        }

        let ptrace_signal_delivery = self.take_ptrace_signal_delivery(signum);
        if self.is_traced() && !ptrace_signal_delivery {
            self.request_ptrace_stop(signum, Some(signal));
            return true;
        }

        if signum == signum::SIGSTOP {
            self.parent().stop_tasks_from_signal(signal);
            return true;
        }

        if signal.si_code == SiCode::SI_TIMER {
            if let KSiFields::Timer(timer_info) = signal.fields {
                if timer_info.si_tid >= 0 {
                    if let Some(timer) = self.parent().timers.get(timer_info.si_tid as usize) {
                        timer.set_delivered_overrun(timer_info.si_overrun.max(0) as u64);
                    }
                }
            }
        }

        let action = self.parent().signal_actions().lock().get(signal.signum);

        if action.is_default() {
            match signum.default_action() {
                SignalDefaultAction::Term => {
                    self.parent().exit(ExitStatus::Signal {
                        sig: signum.num() as u8,
                        coredump: false,
                    });
                    return true;
                }
                SignalDefaultAction::Stop => {
                    self.parent().stop_tasks_from_signal(signal);
                    return true;
                }
                SignalDefaultAction::Core => {
                    self.parent().exit(ExitStatus::Signal {
                        sig: signum.num() as u8,
                        coredump: true,
                    });
                    return true;
                }
                _ => return false,
            }
        } else if action.is_ignore() {
            return false;
        }

        let old_mask = self.get_signal_mask();
        let mut new_mask = old_mask | action.mask;
        if !action.flags.contains(SignalActionFlags::SA_NODEFER) {
            new_mask |= signum.to_mask_set();
        }
        self.set_signal_mask(new_mask);

        let user_context = self.user_context();

        if action.flags.contains(SignalActionFlags::SA_RESTART) {
            // skipped in syscall return, move back
            if let Some(retreg) = self.pop_ucontext_syscall_retreg() {
                user_context.move_back_to_syscall_instruction();
                user_context.set_syscall_retval(retreg);
            }
        }

        let mut sigframe = SigFrame::empty();
        sigframe.info.si_signo = Into::<u32>::into(signum) as i32;
        sigframe.info.si_errno = signal.si_errno;
        sigframe.info.si_code = signal.si_code;
        sigframe.info.fields = signal.fields.into();
        sigframe.ucontext._uc_stack = SignalStack::from_state(self.get_signal_stack_state());
        sigframe.ucontext.uc_sigmask = old_mask;
        sigframe.ucontext.uc_mcontext = (*user_context).into();

        let mut stack_top = self.user_context().get_user_stack_top();
        if action.flags.contains(SignalActionFlags::SA_ONSTACK) {
            let mut signal_stack = self.get_signal_stack_state();
            if !signal_stack.on_stack {
                if let Some(altstack_top) = signal_stack.get_stack_top() {
                    signal_stack.on_stack = true;
                    self.set_signal_stack_state(signal_stack);
                    stack_top = altstack_top;
                }
            }
        }
        stack_top -= core::mem::size_of::<SigFrame>();
        stack_top &= !0xf; // Align to 16 bytes
        self.get_addrspace()
            .copy_to_user(stack_top, sigframe)
            .expect("Failed to copy sigframe to user stack");

        let user_context = self.user_context();
        user_context
            .set_sigaction_restorer(vdso::addr_of("sigreturn_trampoline") + config::VDSO_BASE)
            .set_arg(0, signum.into())
            .set_user_entry(action.handler)
            .set_user_stack_top(stack_top);

        if action.flags.contains(SignalActionFlags::SA_SIGINFO) {
            let siginfo_uaddr = stack_top + core::mem::offset_of!(SigFrame, info);
            let ucontext_uaddr = stack_top + core::mem::offset_of!(SigFrame, ucontext);
            user_context.set_arg(1, siginfo_uaddr).set_arg(2, ucontext_uaddr);
        }

        true
    }

    pub fn return_from_signal(&self) {
        let sigframe = self
            .get_addrspace()
            .copy_from_user::<SigFrame>(self.user_context().get_user_stack_top())
            .expect("Failed to copy sigframe from user stack");
        self.set_signal_stack_state(sigframe.ucontext._uc_stack.into_state());
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

            match scheduler::wakeup_task(self.clone(), Event::WaitSignal { signum }) {
                Ok(()) => return true,
                Err(WakeupFailure::NotBlocked) => return false,
                Err(WakeupFailure::BlockedUninterruptible) => return false,
            }
        }

        if state.pending_signal.is_some() {
            return false;
        }

        if signum == signum::SIGSTOP && state.stop_pending() {
            return true;
        }

        if signum.is_unignorable() {
            state.pending_signal = Some(pending);
            drop(state);

            return match scheduler::wakeup_task(self.clone(), Event::Signal) {
                Ok(()) | Err(WakeupFailure::NotBlocked) => true,
                Err(WakeupFailure::BlockedUninterruptible) => false,
            };
        }

        let mask = self.get_signal_mask();
        if !signum.is_masked(mask) {
            state.pending_signal = Some(pending);
            drop(state);

            let action = self.parent().signal_actions().lock().get(signum);

            if action.is_default() {
                match signum.default_action() {
                    SignalDefaultAction::Ign => return true,
                    _ => {}
                }
            } else if action.is_ignore() {
                return true;
            };

            return match scheduler::wakeup_task(self.clone(), Event::Signal) {
                Ok(()) | Err(WakeupFailure::NotBlocked) => true,
                Err(WakeupFailure::BlockedUninterruptible) => false,
            };
        } else {
            false
        }
    }

    pub fn recive_pending_signal_from_parent(&self) {
        let mask = *self.signal_mask.lock();
        let mut state = self.state().lock();

        if state.pending_signal.is_some() {
            drop(state);
            return;
        }

        if let Some(signal) = self.parent().pending_signals().lock().pop_pending(mask, self.tid()) {
            state.pending_signal = Some(signal);
        }
    }
}

impl PCB {
    fn stop_tasks_from_signal(&self, pending: PendingSignal) {
        let signum = pending.signum;
        let tasks = self.tasks.lock();
        tasks.iter().for_each(|task| {
            if task.is_traced() {
                if task.request_ptrace_stop(signum, Some(pending)) {
                    let _ = scheduler::wakeup_task(task.clone(), Event::Signal);
                }
            } else if task.request_stop_from_signal(signum) {
                let _ = scheduler::wakeup_task(task.clone(), Event::Signal);
            }
        });
    }

    fn resume_stopped_tasks(&self) {
        let mut resumed = false;
        let tasks = self.tasks.lock();
        tasks.iter().for_each(|task| {
            if task.resume_from_stopped() {
                resumed = true;
                scheduler::push_task(task.clone());
            }
        });
        drop(tasks);

        if resumed {
            self.notify_continued();
        }
    }

    pub fn send_signal(
        &self,
        signum: SignalNum,
        si_code: SiCode,
        si_errno: i32,
        fields: KSiFields,
        dest: Option<Tid>,
    ) -> SysResult<()> {
        let pending = PendingSignal {
            signum,
            si_errno,
            si_code,
            fields,
            dest,
        };

        if signum == signum::SIGSTOP {
            if let Some(dest) = dest {
                let tasks = self.tasks.lock();
                if !tasks.iter().any(|task| task.tid() == dest) {
                    return Err(Errno::ESRCH);
                }
            }

            self.stop_tasks_from_signal(pending);

            return Ok(());
        }

        if let Some(dest) = dest {
            let task = {
                let tasks = self.tasks.lock();
                tasks.iter().find(|t| t.tid() == dest).cloned()
            };

            if let Some(task) = task {
                if signum.is_continue() {
                    self.resume_stopped_tasks();
                } else if signum.is_kill() && task.resume_from_stopped() {
                    // push to ready queue to make it exit by itself.
                    scheduler::push_task(task.clone());
                }
                if !task.try_recive_pending_signal(pending) {
                    self.pending_signals().lock().add_pending(pending)?;
                }
                return Ok(());
            } else {
                return Err(Errno::ESRCH);
            }
        }

        if signum.is_continue() {
            self.resume_stopped_tasks();
        }

        let tasks = self.tasks.lock();

        if signum.is_kill() {
            tasks.iter().for_each(|task| {
                if task.resume_from_stopped() {
                    scheduler::push_task(task.clone());
                }
            });
        }

        for task in tasks.iter() {
            if task.try_recive_pending_signal(pending) {
                return Ok(());
            }
        }

        self.pending_signals().lock().add_pending(pending)?;

        Ok(())
    }
}
