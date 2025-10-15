use alloc::sync::Arc;

use crate::arch::UserContextTrait;
use crate::kernel::event::Event;
use crate::kernel::ipc::signal::frame::SigFrame;
use crate::kernel::task::{TCB, PCB, Tid};
use crate::kernel::scheduler::current;
use crate::kernel::errno::{SysResult, Errno};
use crate::kinfo;

use super::{SignalNum, PendingSignal, SignalDefaultAction, SignalActionFlags};

impl TCB {    
    pub fn handle_signal(&self) {
        let mut signal_pending = self.pending_signal.lock();
        let signal = match signal_pending.take() {
            Some(sig) => sig,
            None => return,
        };

        let signum = signal.signum;
        if signum == 0 {
            return;
        }
        if signum == SignalNum::SIGKILL as u32 || signum == SignalNum::SIGSTOP as u32 {
            self.parent.exit(0);
            current::schedule();

            unreachable!();
        }
        
        let action = self.parent.signal_actions().lock().get(signal.signum);
        if action.is_default() {
            match SignalNum::default_action(signum) {
                SignalDefaultAction::Term | SignalDefaultAction::Stop | SignalDefaultAction::Core => {
                    self.parent.exit(0);
                    current::schedule();

                    unreachable!();
                },
                _ => {},
            }
        }

        let old_mask = self.get_signal_mask();
        let mut new_mask = old_mask | action.mask;
        if !action.flags.contains(SignalActionFlags::SA_NODEFER) {
            new_mask |= SignalNum::to_mask(signum);
        }
        self.set_signal_mask(new_mask);

        let mut sigframe = SigFrame::empty();
        sigframe.info.si_signo = signum as i32;
        sigframe.info.si_code = 0;
        sigframe.info.si_errno = 0;
        sigframe.ucontext.uc_sigmask = old_mask;
        sigframe.ucontext.uc_mcontext = (*self.user_context()).into();

        // self.user_context().set_sigaction_restorer(action.restorer).set_user_entry(action.handler);
        self.user_context().set_user_entry(action.handler);

        // kinfo!("Deliver signal {} to task {}, handler={:#x}, restorer={:#x}", signum, self.tid, action.handler, action.restorer);
        
        let mut stack_top = self.user_context().get_user_stack_top();
        stack_top -= core::mem::size_of::<SigFrame>();
        stack_top &= !0xf; // Align to 16 bytes
        self.get_addrspace().copy_to_user(stack_top, unsafe {
            core::slice::from_raw_parts((&sigframe as *const SigFrame) as *const u8, core::mem::size_of::<SigFrame>())
        }).expect("Failed to copy sigframe to user stack");
        self.user_context().set_user_stack_top(stack_top);
    }

    pub fn return_from_signal(&self) {
        let sigframe = self.get_addrspace().copy_from_user_type::<SigFrame>(self.user_context().get_user_stack_top())
            .expect("Failed to copy sigframe from user stack");
    }

    pub fn send_pending_signal(self: &Arc<Self>, pending: PendingSignal) -> bool {
        let mut task_pending = self.pending_signal.lock();
        if task_pending.is_some() {
            return false;
        }
        
        let signum = pending.signum;
        if SignalNum::is_unignorable(signum) {
            *task_pending = Some(pending);
            self.wakeup(Event::Signal);
            kinfo!("Send unignorable signal {} to task {}", signum, self.tid);

            return true;
        }

        let mask = self.get_signal_mask();
        if !SignalNum::is_masked(signum, mask) {
            *task_pending = Some(pending);
            self.wakeup(Event::Signal);
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
