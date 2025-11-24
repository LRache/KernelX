use crate::kernel::task::Pid;
use crate::kernel::uapi::uid_t;

const SI_PAD_SIZE: usize = 128 - 3 * core::mem::size_of::<i32>();

#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct SiKill {
    pub si_pid: Pid,   // Sending process ID
    pub si_uid: uid_t, // Real user ID of sending process
}

#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct SiTimer {
    pub si_tid: i32,      // Timer ID
    pub si_overrun: i32,  // Overrun count
    pub si_sigval: usize, // Signal value
}

#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct SiSigChld {
    pub si_pid: Pid,     // Child process ID
    pub si_uid: uid_t,   // Real user ID of sending process
    pub si_status: i32,  // Exit value or signal
    pub si_utime: usize, // User time consumed
    pub si_stime: usize, // System time consumed
}

#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct SiSigFault {
    pub si_addr: usize,   // Faulting instruction/memory reference
    pub si_addr_lsb: i16, // Valid LSBs of si_addr
}

#[repr(transparent)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct SiCode(pub i32);

impl SiCode {
    pub const EMPTY: Self = Self(0);
    pub const SI_USER: Self = Self(0);
    pub const SI_KERNEL: Self = Self(0x80);
    pub const SI_QUEUE: Self = Self(-1);
    pub const SI_TIMER: Self = Self(-2);
    pub const SI_TKILL: Self = Self(-6);
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

#[derive(Clone, Copy, Debug)]
pub enum KSiFields {
    Empty,
    Kill(SiKill),
    Timer(SiTimer),
    SigChld(SiSigChld),
    SigFault(SiSigFault),
}

impl KSiFields {
    pub fn kill(pid: Pid, uid: uid_t) -> Self {
        Self::Kill(SiKill {
            si_pid: pid,
            si_uid: uid,
        })
    }
}

impl Into<USiFields> for KSiFields {
    fn into(self) -> USiFields {
        match self {
            KSiFields::Empty => USiFields {
                _pad: [0; SI_PAD_SIZE / core::mem::size_of::<i32>()],
            },
            KSiFields::Kill(kill) => USiFields { kill },
            KSiFields::SigChld(sigchld) => USiFields { sigchld },
            KSiFields::SigFault(sigfault) => USiFields { sigfault },
            KSiFields::Timer(timer) => USiFields { timer },
        }
    }
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct SigInfo {
    pub si_signo: i32,   // Signal number
    pub si_errno: i32,   // An errno value
    pub si_code: SiCode, // Signal code
    __pad0: i32,
    pub fields: USiFields,
}

impl SigInfo {
    pub fn empty() -> Self {
        SigInfo {
            si_signo: 0,
            si_errno: 0,
            si_code: SiCode::EMPTY,
            __pad0: 0,
            fields: USiFields {
                _pad: [0; SI_PAD_SIZE / core::mem::size_of::<i32>()],
            },
        }
    }
}
