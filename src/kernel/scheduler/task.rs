use crate::arch;
use crate::kernel::event::Event;
use crate::kernel::mm;
use crate::kernel::mm::MapPerm;
use crate::kernel::task::TCB;

use super::Tid;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TaskState {
    /// The task is currently running on a CPU.
    Running,

    /// The task is ready to run and can be scheduled on a CPU.
    Ready,

    /// The task is blocked, waiting for an event.
    Blocked,

    /// The task is blocked and cannot be interrupted until the event it is waiting for occurs.
    BlockedUninterruptible,

    /// The task has exited. This state MUST BE set by the task itself.
    Exited,
}

pub struct KernelStack {
    top: usize,
    page_count: usize,
}

impl KernelStack {
    pub fn new(page_count: usize) -> Self {
        let base = mm::page::alloc_contiguous(page_count + 1);
        let top = base + arch::PGSIZE * (page_count + 1);
        unsafe { arch::unmap_kernel_addr(base, arch::PGSIZE) };
        Self { top, page_count }
    }

    pub fn get_top(&self) -> usize {
        self.top
    }

    pub fn check_stack_overflow(&self, sp: usize) -> bool {
        let base = self.top - arch::PGSIZE * (self.page_count + 1);
        sp < self.top && sp >= base
    }
}

impl Drop for KernelStack {
    fn drop(&mut self) {
        let base = self.top - arch::PGSIZE * (self.page_count + 1);
        arch::map_kernel_addr(base, arch::kaddr_to_paddr(base), arch::PGSIZE, MapPerm::RW);
        mm::page::free_contiguous(base, self.page_count + 1);
    }
}

unsafe impl Send for KernelStack {}
unsafe impl Sync for KernelStack {}

pub enum WakeupFailure {
    /// The task is not blocked.
    NotBlocked,

    /// The task is blocked, but the event it is waiting for does not match the given event.
    BlockedUninterruptible,
}

pub trait Task: Send + Sync {
    fn tid(&self) -> Tid;
    fn kcontext(&self) -> &mut arch::KernelContext;
    fn kstack(&self) -> &KernelStack;

    fn run_if_ready(&self) -> bool;
    fn state_running_to_ready(&self) -> bool;

    fn block(&self, reason: &str) -> bool;
    fn block_uninterruptible(&self, reason: &str) -> bool;
    fn unblock(&self);

    fn wakeup(&self, event: Event) -> Result<(), WakeupFailure>;
    fn wakeup_uninterruptible(&self, event: Event) -> bool;
    fn take_wakeup_event(&self) -> Option<Event>;

    fn tcb(&self) -> &TCB;

    fn set_exited(&self) {}

    #[cfg(feature = "deadlock-detect")]
    fn lockstate(&self) -> &crate::klib::ksync::LockState;
}
