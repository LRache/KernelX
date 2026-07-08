use alloc::vec::Vec;

use crate::arch;
use crate::kernel::event::Event;
use crate::kernel::mm;
use crate::kernel::mm::MapPerm;
use crate::kernel::task::TCB;

use crate::kernel::uapi::Uid;

use super::Tid;

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

pub enum WakeupFailure {
    /// The task is not blocked.
    NotBlocked,

    /// The task is blocked, but the event it is waiting for does not match the given event.
    BlockedUninterruptible,
}

pub trait Task: Send + Sync {
    fn tid(&self) -> Tid;
    fn euid(&self) -> Uid;
    fn egid(&self) -> Uid;
    fn fsuid(&self) -> Uid;
    fn fsgid(&self) -> Uid;
    fn supplementary_gids(&self) -> Vec<Uid> {
        Vec::new()
    }

    /// Raw saved kernel context pointer used by context switching.
    ///
    /// Callers must not dereference it unless they own this task's saved context.
    fn kcontext_ptr(&self) -> *mut arch::KernelContext;
    fn kstack(&self) -> &KernelStack;

    fn run_if_ready(&self) -> bool;
    fn state_running_to_ready(&self) -> bool;

    fn block(&self, reason: &str) -> bool;
    fn block_uninterruptible(&self, reason: &str) -> bool;
    fn unblock(&self);

    fn wakeup(&self, event: Event) -> Result<(), WakeupFailure>;
    fn wakeup_uninterruptible(&self, event: Event) -> bool;
    fn take_wakeup_event(&self) -> Option<Event>;

    fn pause_system_time(&self) {}
    fn resume_system_time(&self) {}

    fn tcb(&self) -> &TCB;

    fn set_exited(&self) {}

    #[cfg(feature = "lockdep")]
    fn lockstate(&self) -> &crate::klib::ksync::LockState;
}
