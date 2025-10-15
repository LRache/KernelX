use crate::kernel::api::uid_t;
use crate::kernel::ipc::SignalSet;
use crate::kernel::task::Pid;
use crate::arch::SigContext;

const SI_PAD_SIZE: usize = 128 - 3 * core::mem::size_of::<i32>();

#[repr(C)]
#[derive(Clone, Copy)]
pub struct SiKill {
    pub si_pid: Pid,        // Sending process ID
    pub si_uid: uid_t, // Real user ID of sending process
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct SiTimer {
    pub si_tid: i32,       // Timer ID
    pub si_overrun: i32,   // Overrun count
    pub si_sigval: usize,  // Signal value
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct SiSigChld {
    pub si_pid: Pid,     // Child process ID
    pub si_uid: uid_t,   // Real user ID of sending process
    pub si_status: i32,  // Exit value or signal
    pub si_utime: usize, // User time consumed
    pub si_stime: usize, // System time consumed
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct SiSigFault {
    pub si_addr: usize,    // Faulting instruction/memory reference
    pub si_addr_lsb: i16,  // Valid LSBs of si_addr
}

#[repr(C)]
#[derive(Clone, Copy)]
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
#[derive(Clone, Copy)]
pub struct SigInfo {
    pub si_signo: i32,   // Signal number
    pub si_errno: i32,   // An errno value
    pub si_code: i32,    // Signal code
    pub _fields: SiFields,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct SignalStack {
    pub ss_sp: usize,
    pub ss_flags: i32,
    pub ss_size: usize,
}

#[repr(C)]
#[derive(Clone, Copy)]
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
                __unused: [0; 1024 / 8 - core::mem::size_of::<SignalSet>()],
                uc_mcontext: SigContext::empty(),
            },
        }
    }
}
