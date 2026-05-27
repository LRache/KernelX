use crate::kernel::ipc::SignalNum;
use crate::kernel::scheduler::Tid;

use super::FileEvent;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Event {
    Poll { event: FileEvent, waker: usize },
    Epoll,
    ReadReady,
    WriteReady,
    Timeout,
    Futex,
    FutexWaitv { index: usize },
    Sem,
    Process { child: Tid },
    WaitSignal { signum: SignalNum },
    Signal,
    Ptrace,
    FanotifyPermission,
    VFork,
    IOComplete,
    SleepLock,
}
