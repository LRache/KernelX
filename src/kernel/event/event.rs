use crate::kernel::{ipc::SignalNum, task::Tid};

use super::PollEvent;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Event {
    Poll { event: PollEvent, waker: usize },
    PipeReadReady,
    PipeWriteReady,
    Timeout,
    Futex, 
    Process { child: Tid },
    WaitSignal { signum: SignalNum },
    Signal,
    VFork,
}
