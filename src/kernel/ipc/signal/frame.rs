use crate::arch::SigContext;
use crate::kernel::ipc::SignalSet;

use super::siginfo::SigInfo;
use super::{SignalStackFlags, SignalStackState};

#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct SignalStack {
    pub ss_sp: usize,
    pub ss_flags: i32,
    pub ss_size: usize,
}

impl SignalStack {
    pub const fn empty() -> Self {
        Self {
            ss_sp: 0,
            ss_flags: SignalStackFlags::SS_DISABLE.bits() as i32,
            ss_size: 0,
        }
    }

    pub fn from_state(state: SignalStackState) -> Self {
        if let Some((ss_sp, ss_size)) = state.stack {
            let mut flags = SignalStackFlags::empty();
            if state.on_stack {
                flags |= SignalStackFlags::SS_ONSTACK;
            }
            Self {
                ss_sp,
                ss_flags: flags.bits() as i32,
                ss_size,
            }
        } else {
            Self::empty()
        }
    }

    pub fn into_state(self) -> SignalStackState {
        let flags = SignalStackFlags::from_bits_truncate(self.ss_flags as usize);
        SignalStackState {
            stack: if flags.contains(SignalStackFlags::SS_DISABLE) {
                None
            } else {
                Some((self.ss_sp, self.ss_size))
            },
            on_stack: flags.contains(SignalStackFlags::SS_ONSTACK),
        }
    }
}

#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct SignalUContext {
    pub _uc_flags: usize,       // 8
    pub _uc_link: usize,        // 16
    pub _uc_stack: SignalStack, // 16 + 24 = 40
    pub uc_sigmask: SignalSet,  // 48
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
                _uc_stack: SignalStack::empty(),
                uc_sigmask: SignalSet::empty(),
                __unused: [0; 128 - core::mem::size_of::<SignalSet>()],
                uc_mcontext: SigContext::empty(),
            },
        }
    }
}
