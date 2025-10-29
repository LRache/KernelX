use crate::kernel::ipc::SignalSet;
use crate::arch::SigContext;

use super::siginfo::SigInfo;

#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct SignalStack {
    pub ss_sp: usize,
    pub ss_flags: i32,
    pub ss_size: usize,
}

#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct SignalUContext {
    pub _uc_flags: usize,
    pub _uc_link:  usize,
    pub _uc_stack: SignalStack,
    pub uc_sigmask: SignalSet,
    pub __unused: [u8; 1024 / 8 - core::mem::size_of::<SignalSet>()],
    pub uc_mcontext: SigContext,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct SigFrame {
    pub info: SigInfo,
    pub ucontext: SignalUContext,
}

impl SigFrame {
    pub fn empty() -> Self {
        SigFrame {
            info: SigInfo::empty(),
            ucontext: SignalUContext {
                _uc_flags: 0,
                _uc_link: 0,
                _uc_stack: SignalStack {
                    ss_sp: 0,
                    ss_flags: 0,
                    ss_size: 0,
                },
                uc_sigmask: SignalSet::empty(),
                __unused: [0; 1024 / 8 - core::mem::size_of::<SignalSet>()],
                uc_mcontext: SigContext::empty(),
            },
        }
    }
}
