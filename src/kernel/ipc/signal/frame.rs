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
    pub _uc_flags: usize, // 8
    pub _uc_link:  usize, // 16
    pub _uc_stack: SignalStack, // 16 + 24 = 40
    pub uc_sigmask: SignalSet, // 48
    pub __unused: [u8; 128 - core::mem::size_of::<SignalSet>()],
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
                __unused: [0; 128 - core::mem::size_of::<SignalSet>()],
                uc_mcontext: SigContext::empty(),
            },
        }
    }
}
