use crate::kernel::task::Pid;
use crate::kernel::uapi::uid_t;

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

#[derive(Clone, Copy)]
pub enum SiFields {
    Empty,
    Kill(SiKill),
    Timer(SiTimer),
    SigChld(SiSigChld),
    SigFault(SiSigFault),
}

#[repr(C)]
#[derive(Clone, Copy)]
pub union USiFields {
    _pad: [i32; SI_PAD_SIZE / core::mem::size_of::<i32>()],
    kill: SiKill,
    timer: SiTimer,
    sigchld: SiSigChld,
    sigfault: SiSigFault,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct SigInfo {
    pub si_signo: i32,   // Signal number
    pub si_errno: i32,   // An errno value
    pub si_code:  i32,   // Signal code
    pub fields: USiFields,
}

impl SigInfo {
    pub fn empty() -> Self {
        SigInfo {
            si_signo: 0,
            si_errno: 0,
            si_code: 0,
            fields: USiFields { _pad: [0; SI_PAD_SIZE / core::mem::size_of::<i32>()] },
        }
    }
}
