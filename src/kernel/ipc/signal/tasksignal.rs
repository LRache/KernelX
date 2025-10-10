use crate::kernel::api;
use crate::kernel::errno::{Errno, SysResult};
use crate::kernel::scheduler::current;
use crate::kernel::task::TCB;
use crate::arch::UserContextTrait;

#[derive(Clone, Copy)]
pub struct SigactionItem {
    pub handler: usize,
    pub restorer: usize,
    pub mask: api::sigset_t,
}

pub struct TaskSignal {
    pub sigactions: [SigactionItem; 32],
    pub pending: api::sigset_t,
    pub blocked: api::sigset_t,
}

const SIG_DFL: usize = 0;
const SIG_IGN: usize = 1;

impl TaskSignal {
    pub fn new() -> Self {
        TaskSignal {
            sigactions: [SigactionItem {
                handler: SIG_DFL,
                restorer: 0,
                mask: 0,
            }; 32],
            pending: 0,
            blocked: 0,
        }
    }

    pub fn get_sigaction(&self, signum: u32) -> SysResult<SigactionItem> {
        if signum > 32 {
            return Err(Errno::EINVAL)?;
        }
        Ok(self.sigactions[signum as usize])
    }

    pub fn set_sigaction(&mut self, signum: u32, action: &SigactionItem) -> SysResult<()> {
        if signum > 32 {
            return Err(Errno::EINVAL)?;
        }
        self.sigactions[signum as usize] = *action;

        Ok(())
    }

    pub fn add_pending(&mut self, signum: u32) -> SysResult<()> {
        if signum > 32 {
            return Err(Errno::EINVAL)?;
        }
        let bit = 1 << signum;
        self.pending |= bit;
        Ok(())
    }

    pub fn handle_signal(&mut self, tcb: &TCB) -> Option<u32> {
        let to_handle = self.pending & self.blocked;
        if to_handle == 0 {
            return None;
        }
        
        for sig in 0..api::SIGMAX {
            let bit = 1 << sig;
            if to_handle & bit == 0 {
                continue;
            }

            self.pending &= !bit;

            if sig == api::SignalNum::SIGKILL as u32 || sig == api::SignalNum::SIGSTOP as u32 {
                tcb.parent.exit(0);
                current::schedule();
                unreachable!();
            }

            let action = self.sigactions[sig as usize];
            if action.handler == SIG_IGN {
                continue;
            }

            if action.handler == SIG_DFL {
                continue;
            }

            tcb.with_user_context_mut(|context| {
                let sigframe = api::SigFrame {
                    info: api::SigInfo {
                        si_signo: sig as i32,
                        si_errno: 0,
                        si_code: 0,
                        _fields: api::SiFields::zero() 
                    },
                    ucontext: api::SignalUContext {
                        _uc_flags: 0,
                        _uc_link: 0,
                        _uc_stack: api::SignalStack {
                            ss_sp: 0,
                            ss_flags: 0,
                            ss_size: 0,
                        },
                        uc_sigmask: self.blocked,
                        __unused: [0; 1024 / 8 - core::mem::size_of::<api::sigset_t>()],
                        _uc_mcontext: (*context).into(),
                    },
                };

                context.set_sigaction_restorer(action.restorer);
                let mut stack_top = context.get_user_stack_top();
                stack_top -= core::mem::size_of::<api::SigFrame>();
                let stack_top = stack_top & !0xf; // Align to 16 bytes
                context.set_user_stack_top(stack_top);
                
                tcb.get_addrspace().copy_to_user(stack_top, unsafe {
                    core::slice::from_raw_parts(
                        &sigframe as *const api::SigFrame as *const u8,
                        core::mem::size_of::<api::SigFrame>(),
                    )
                }).unwrap();

                self.blocked = self.blocked | action.mask | bit;
            });
            
            return Some(sig);
        }
        None
    }

    pub fn signal_return(&mut self, tcb: &TCB) {
        tcb.with_user_context_mut(|context| {
            let stack_top = context.get_user_stack_top();
            
            let mut sigframe = api::SigFrame::empty();
            tcb.get_addrspace().copy_from_user(stack_top, unsafe {
                core::slice::from_raw_parts_mut(
                    &mut sigframe as *mut api::SigFrame as *mut u8,
                    core::mem::size_of::<api::SigFrame>(),
                )
            }).unwrap();

            self.blocked = sigframe.ucontext.uc_sigmask;
            context.restore_from_signal(&sigframe.ucontext._uc_mcontext);
            
            let new_stack_top = stack_top + core::mem::size_of::<api::SigFrame>();
            context.set_user_stack_top(new_stack_top);
        });
    }
}
