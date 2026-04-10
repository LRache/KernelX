use alloc::vec::Vec;

use crate::kernel::ipc::SignalNum;
use crate::kernel::scheduler::Tid;

use super::FileEvent;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Event {
    Poll { event: FileEvent, waker: usize },
    ReadReady,
    WriteReady,
    Timeout,
    Futex,
    Process { child: Tid },
    WaitSignal { signum: SignalNum },
    Signal,
    VFork,
    IOComplete,
    SleepLock,
    Net { packet: Vec<u8> },
}
