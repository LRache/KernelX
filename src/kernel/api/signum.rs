use num_enum::TryFromPrimitive;
use bitflags::bitflags;

#[allow(non_camel_case_types)]
pub type sigset_t = usize;

#[repr(u32)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, TryFromPrimitive)]
pub enum SignalNum {
    SIGHUP  = 1,
    SIGINT  = 2,
    SIGQUIT = 3,
    SIGILL  = 4,
    SIGABRT = 6,
    SIGFPE  = 8,
    SIGKILL = 9,
    SIGSEGV = 11,
    SIGPIPE = 13,
    SIGALRM = 14,
    SIGTERM = 15,
    SIGUSR1 = 10,
    SIGUSR2 = 12,
    SIGCHLD = 17,
    SIGCONT = 18,
    SIGSTOP = 19,
    SIGTSTP = 20,
    SIGTTIN = 21,
    SIGTTOU = 22,
}

pub const SIGMAX: u32 = (core::mem::size_of::<sigset_t>() * 8) as u32;

bitflags! {
    #[repr(C)]
    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    pub struct SignalSet: usize {
        const SIGHUP  = 1 << (SignalNum::SIGHUP  as sigset_t);
        const SIGINT  = 1 << (SignalNum::SIGINT  as sigset_t);
        const SIGQUIT = 1 << (SignalNum::SIGQUIT as sigset_t);
        const SIGILL  = 1 << (SignalNum::SIGILL  as sigset_t);
        const SIGABRT = 1 << (SignalNum::SIGABRT as sigset_t);
        const SIGFPE  = 1 << (SignalNum::SIGFPE  as sigset_t);
        const SIGKILL = 1 << (SignalNum::SIGKILL as sigset_t);
        const SIGSEGV = 1 << (SignalNum::SIGSEGV as sigset_t);
        const SIGPIPE = 1 << (SignalNum::SIGPIPE as sigset_t);
        const SIGALRM = 1 << (SignalNum::SIGALRM as sigset_t);
        const SIGTERM = 1 << (SignalNum::SIGTERM as sigset_t);
        const SIGUSR1 = 1 << (SignalNum::SIGUSR1 as sigset_t);
        const SIGUSR2 = 1 << (SignalNum::SIGUSR2 as sigset_t);
        const SIGCHLD = 1 << (SignalNum::SIGCHLD as sigset_t);
        const SIGCONT = 1 << (SignalNum::SIGCONT as sigset_t);
        const SIGSTOP = 1 << (SignalNum::SIGSTOP as sigset_t);
        const SIGTSTP = 1 << (SignalNum::SIGTSTP as sigset_t);
        const SIGTTIN = 1 << (SignalNum::SIGTTIN as sigset_t);
        const SIGTTOU = 1 << (SignalNum::SIGTTOU as sigset_t);
    }
}
