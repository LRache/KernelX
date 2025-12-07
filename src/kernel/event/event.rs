use crate::kernel::ipc::SignalNum;
use crate::kernel::scheduler::Tid;

use super::FileEvent;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Event {
    Poll { event: FileEvent, waker: usize },
    PipeReadReady,
    PipeWriteReady,
    Timeout,
    Futex, 
    Process { child: Tid },
    WaitSignal { signum: SignalNum },
    Signal,
    VFork,
}
