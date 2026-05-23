use core::time::Duration;

use crate::kernel::ipc::{SignalNum, signum};

#[derive(Clone, Copy)]
pub struct ITimer {
    pub id: u64,
    /// Absolute expiry time in microseconds
    pub expiry_us: u64,
    /// Interval for repeating itimers
    pub interval: Duration,
}

#[derive(Debug, Clone, Copy)]
pub enum ExitStatus {
    /// Normal exit with exit code (from exit/exit_group syscall)
    Normal(u8),
    /// Killed by signal, with optional core dump flag
    Signal { sig: u8, coredump: bool },
}

impl ExitStatus {
    /// Encode as POSIX wait status (wstatus)
    pub fn as_wstatus(self) -> u32 {
        match self {
            ExitStatus::Normal(code) => (code as u32) << 8,
            ExitStatus::Signal { sig, coredump } => {
                let status = sig as u32 & 0x7f;
                if coredump { status | 0x80 } else { status }
            }
        }
    }

    pub fn si_status(self) -> i32 {
        match self {
            ExitStatus::Normal(code) => code as i32,
            ExitStatus::Signal { sig, .. } => sig as i32,
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub enum WaitStatus {
    Exited(ExitStatus),
    Stopped(SignalNum),
    PtraceStopped(SignalNum),
    Continued,
}

impl WaitStatus {
    pub fn as_wstatus(self) -> u32 {
        match self {
            WaitStatus::Exited(status) => status.as_wstatus(),
            WaitStatus::Stopped(signum) | WaitStatus::PtraceStopped(signum) => ((signum.num() & 0xff) << 8) | 0x7f,
            WaitStatus::Continued => 0xffff,
        }
    }

    pub fn si_status(self) -> i32 {
        match self {
            WaitStatus::Exited(status) => status.si_status(),
            WaitStatus::Stopped(signum) | WaitStatus::PtraceStopped(signum) => signum.num() as i32,
            WaitStatus::Continued => signum::SIGCONT.num() as i32,
        }
    }
}

#[derive(Debug)]
pub(super) enum State {
    Running,
    Exited(ExitStatus),
    Recycled,
}

#[derive(Clone, Copy, Debug)]
pub(super) enum ChildWaitStatus {
    Stopped { signum: SignalNum, reported: bool },
    Continued { reported: bool },
}
