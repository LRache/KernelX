use crate::kernel::api::sigset_t;
use crate::arch::SigContext;

use super::{pid_t, uid_t};

type sigval_t = usize;

const SI_PAD_SIZE: usize = 128 - 3 * core::mem::size_of::<i32>();

#[repr(C)]
#[derive(Clone, Copy)]
pub struct SiKill {
    pub si_pid: pid_t,   // Sending process ID
    pub si_uid: uid_t,   // Real user ID of sending process
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct SiTimer {
    pub si_tid: i32,       // Timer ID
    pub si_overrun: i32,   // Overrun count
    pub si_sigval: sigval_t,  // Signal value
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct SiSigChld {
    pub si_pid: pid_t,    // Child process ID
    pub si_uid: uid_t,    // Real user ID of sending process
    pub si_status: i32,   // Exit value or signal
    pub si_utime: usize,  // User time consumed
    pub si_stime: usize,  // System time consumed
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct SiSigFault {
    pub si_addr: usize,    // Faulting instruction/memory reference
    pub si_addr_lsb: i16,  // Valid LSBs of si_addr
}

#[repr(C)]
pub union SiFields {
    _pad: [i32; SI_PAD_SIZE / core::mem::size_of::<i32>()],
    _kill: SiKill,
    _timer: SiTimer,
    _sigchld: SiSigChld,
    _sigfault: SiSigFault,
}

impl SiFields {
    pub fn zero() -> Self {
        SiFields { _pad: [0; SI_PAD_SIZE / core::mem::size_of::<i32>()] }
    }
}

#[repr(C)]
pub struct SigInfo {
    pub si_signo: i32,   // Signal number
    pub si_errno: i32,   // An errno value
    pub si_code: i32,    // Signal code
    pub _fields: SiFields,
}

#[repr(C)]
pub struct SignalStack {
    pub ss_sp: usize,
    pub ss_flags: i32,
    pub ss_size: usize,
}

#[repr(C)] 
pub struct SignalUContext {
    pub _uc_flags: usize,
    pub _uc_link:  usize,
    pub _uc_stack: SignalStack,
    pub uc_sigmask: sigset_t,
    pub __unused: [u8; 1024 / 8 - core::mem::size_of::<sigset_t>()],
    pub _uc_mcontext: SigContext,
}

#[repr(C)]
pub struct SigFrame {
    pub info: SigInfo,
    pub ucontext: SignalUContext,
}

impl SigFrame {
    pub fn empty() -> Self {
        SigFrame {
            info: SigInfo {
                si_signo: 0,
                si_errno: 0,
                si_code: 0,
                _fields: SiFields::zero(),
            },
            ucontext: SignalUContext {
                _uc_flags: 0,
                _uc_link: 0,
                _uc_stack: SignalStack {
                    ss_sp: 0,
                    ss_flags: 0,
                    ss_size: 0,
                },
                uc_sigmask: 0,
                __unused: [0; 1024 / 8 - core::mem::size_of::<sigset_t>()],
                _uc_mcontext: SigContext::empty(),
            },
        }
    }
}
