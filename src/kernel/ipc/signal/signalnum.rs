use num_enum::TryFromPrimitive;

pub enum SignalDefaultAction {
    Core, // Create core dump and terminate
    Term, // Terminate
    Ign,  // Ignore
    Cont, // Continue if stopped
    Stop, // Stop the process
}

#[repr(u32)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, TryFromPrimitive)]
pub enum SignalNum {
    SIGHUP    = 1,
    SIGINT    = 2,
    SIGQUIT   = 3,
    SIGILL    = 4,
    SIGTRAP   = 5,
    SIGABRT   = 6,
    SIGBUS    = 7,
    SIGFPE    = 8,
    SIGKILL   = 9,
    SIGUSR1   = 10,
    SIGSEGV   = 11,
    SIGUSR2   = 12,
    SIGPIPE   = 13,
    SIGALRM   = 14,
    SIGTERM   = 15,
    SIGSTKFLT = 16,
    SIGCHLD   = 17,
    SIGCONT   = 18,
    SIGSTOP   = 19,
    SIGTSTP   = 20,
    SIGTTIN   = 21,
    SIGTTOU   = 22,
    SIGURG    = 23,
    SIGXCPU   = 24,
    SIGXFSZ   = 25,
    SIGVTALRM = 26,
    SIGPROF   = 27,
    SIGWINCH  = 28,
    SIGIO     = 29,
    SIGPWR    = 30,
    SIGSYS    = 31,
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

    pub fn default_action(num: u32) -> SignalDefaultAction {
        let num = match SignalNum::try_from(num) {
            Ok(n) => n,
            Err(_) => return SignalDefaultAction::Ign,
        };
        match num {
            SignalNum::SIGQUIT | SignalNum::SIGILL | SignalNum::SIGABRT | SignalNum::SIGFPE | SignalNum::SIGSEGV |
            SignalNum::SIGBUS | SignalNum::SIGSYS | SignalNum::SIGTRAP | SignalNum::SIGXCPU | SignalNum::SIGXFSZ => SignalDefaultAction::Core,

            SignalNum::SIGSTOP | SignalNum::SIGTSTP | SignalNum::SIGTTIN | SignalNum::SIGTTOU => SignalDefaultAction::Stop,
            
            SignalNum::SIGCONT => SignalDefaultAction::Cont,
            
            SignalNum::SIGCHLD | SignalNum::SIGURG | SignalNum::SIGWINCH => SignalDefaultAction::Ign,
            
            _ => SignalDefaultAction::Term,
        }
    }
}

pub type SignalSet = usize;
