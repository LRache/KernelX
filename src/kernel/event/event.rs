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
    Msg,
    Process { child: Tid },
    WaitSignal { signum: SignalNum },
    Signal,
    FanotifyPermission,
    VFork,
    IOComplete,
    SleepLock,
}
