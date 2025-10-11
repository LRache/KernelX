use crate::kernel::task::Tid;

use super::PollEvent;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Event {
    Poll { event: PollEvent, waker: usize },
    PipeReadReady,
    PipeWriteReady,
    Timeout,
    Process { child: Tid },
    Signal,
}
