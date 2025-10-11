use num_enum::TryFromPrimitive;

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

impl SignalNum {
    pub fn is_unignorable(num: u32) -> bool {
        num == SignalNum::SIGKILL as u32 || num == SignalNum::SIGSTOP as u32
    }

    pub fn to_mask(num: u32) -> usize {
        1 << (num - 1)
    }

    pub fn is_masked(num: u32, mask: usize) -> bool {
        (mask & Self::to_mask(num)) != 0
    }
}

pub type SignalSet = usize;
