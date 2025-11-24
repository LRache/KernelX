use crate::arch;
use crate::kernel::event::Event;
use crate::kernel::task::{TCB, Tid};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TaskState {
    Running,
    Ready,
    Blocked,
    BlockedUninterruptible,
    Exited,
}

pub trait Task: Send + Sync {
    fn tid(&self) -> Tid;
    fn get_kcontext_ptr(&self) -> *mut arch::KernelContext;
    
    fn run_if_ready(&self) -> bool;
    fn state_running_to_ready(&self) -> bool;

    fn block(&self, reason: &str) -> bool;
    fn block_uninterruptible(&self, reason: &str) -> bool;

    fn wakeup(&self, event: Event) -> bool;
    fn wakeup_uninterruptible(&self, event: Event);
    fn take_wakeup_event(&self) -> Option<Event>;
    
    fn tcb(&self) -> &TCB;
}
