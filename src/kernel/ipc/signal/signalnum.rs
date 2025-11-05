use core::ops::{BitAnd, BitOr, BitOrAssign, Not};

use crate::kernel::errno::{Errno, SysResult};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SignalDefaultAction {
    Core, // Create core dump and terminate
    Term, // Terminate
    Ign,  // Ignore
    Cont, // Continue if stopped
    Stop, // Stop the process
}

pub mod signum {
    use super::SignalNum;
    
    pub const SIGHUP: SignalNum = SignalNum(1);
    pub const SIGINT: SignalNum = SignalNum(2);
    pub const SIGQUIT: SignalNum = SignalNum(3);
    pub const SIGILL: SignalNum = SignalNum(4);
    pub const SIGTRAP: SignalNum = SignalNum(5);
    pub const SIGABRT: SignalNum = SignalNum(6);
    pub const SIGBUS: SignalNum = SignalNum(7);
    pub const SIGFPE: SignalNum = SignalNum(8);
    pub const SIGKILL: SignalNum = SignalNum(9);
    pub const SIGUSR1: SignalNum = SignalNum(10);
    pub const SIGSEGV: SignalNum = SignalNum(11);
    pub const SIGUSR2: SignalNum = SignalNum(12);
    pub const SIGPIPE: SignalNum = SignalNum(13);
    pub const SIGALRM: SignalNum = SignalNum(14);
    pub const SIGTERM: SignalNum = SignalNum(15);
    pub const SIGSTKFLT: SignalNum = SignalNum(16);
    pub const SIGCHLD: SignalNum = SignalNum(17);
    pub const SIGCONT: SignalNum = SignalNum(18);
    pub const SIGSTOP: SignalNum = SignalNum(19);
    pub const SIGTSTP: SignalNum = SignalNum(20);
    pub const SIGTTIN: SignalNum = SignalNum(21);
    pub const SIGTTOU: SignalNum = SignalNum(22);
    pub const SIGURG: SignalNum = SignalNum(23);
    pub const SIGXCPU: SignalNum = SignalNum(24);
    pub const SIGXFSZ: SignalNum = SignalNum(25);
    pub const SIGVTALRM: SignalNum = SignalNum(26);
    pub const SIGPROF: SignalNum = SignalNum(27);
    pub const SIGWINCH: SignalNum = SignalNum(28);
    pub const SIGIO: SignalNum = SignalNum(29);
    pub const SIGPWR: SignalNum = SignalNum(30);
    pub const SIGSYS: SignalNum = SignalNum(31);
}


#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SignalNum(u32);

use signum::*;

impl SignalNum {
    pub fn is_empty(&self) -> bool {
        self.0 == 0
    }

    pub fn is_kill(&self) -> bool {
        *self == SIGKILL || *self == SIGSTOP
    }

    pub fn is_unignorable(&self) -> bool {
        *self == SIGKILL || *self == SIGSTOP
    }

    pub fn to_mask(&self) -> usize {
        1 << (self.0 - 1)
    }
    
    pub fn to_mask_set(&self) -> SignalSet {
        SignalSet(self.to_mask())
    }

    pub fn is_masked(&self, set: SignalSet) -> bool {
        (set.0 & self.to_mask()) != 0
    }

    pub fn default_action(&self) -> SignalDefaultAction {
        match *self {
            SIGQUIT | SIGILL | SIGABRT | SIGFPE  | SIGSEGV |
            SIGBUS | SIGSYS  | SIGTRAP | SIGXCPU | SIGXFSZ => SignalDefaultAction::Core,

            SIGSTOP | SIGTSTP | SIGTTIN | SIGTTOU => SignalDefaultAction::Stop,
            
            SIGCONT => SignalDefaultAction::Cont,
            
            SIGCHLD | SIGURG | SIGWINCH => SignalDefaultAction::Ign,
            
            _ => SignalDefaultAction::Term,
        }
    }

    pub fn num(&self) -> u32 {
        self.0
    }
}

impl Into<u32> for SignalNum {
    fn into(self) -> u32 {
        self.0
    }
}

impl TryFrom<u32> for SignalNum {
    type Error = Errno;
    fn try_from(value: u32) -> SysResult<Self> {
        if value > 63 {
            Err(Errno::EINVAL)
        } else {
            Ok(SignalNum(value))
        }
    }
}

impl Into<usize> for SignalNum {
    fn into(self) -> usize {
        self.0 as usize
    }
}

#[derive(Debug, Clone, Copy)]
#[repr(C)]
pub struct SignalSet(usize);

impl SignalSet {
    pub const fn empty() -> Self {
        SignalSet(0)
    }
    
    pub fn contains(&self, num: SignalNum) -> bool {
        num.is_masked(*self)
    }

    pub fn bits(&self) -> usize {
        self.0
    }
}

impl BitOr for SignalSet {
    type Output = SignalSet;

    fn bitor(self, rhs: SignalSet) -> SignalSet {
        SignalSet(self.0 | rhs.0)
    }
}

impl BitOrAssign for SignalSet {
    fn bitor_assign(&mut self, rhs: SignalSet) {
        self.0 |= rhs.0;
    }
}

impl Not for SignalSet {
    type Output = SignalSet;

    fn not(self) -> SignalSet {
        SignalSet(!self.0)
    }
}

impl BitAnd for SignalSet {
    type Output = SignalSet;

    fn bitand(self, rhs: SignalSet) -> SignalSet {
        SignalSet(self.0 & rhs.0)
    }
}

impl Into<usize> for SignalSet {
    fn into(self) -> usize {
        self.0
    }
}
