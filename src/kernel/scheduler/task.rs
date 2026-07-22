use alloc::vec::Vec;

use crate::arch;
pub use crate::arch::KernelStack;
use crate::kernel::event::Event;
use crate::kernel::task::TCB;
use crate::kernel::uapi::Uid;

use super::Tid;

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
    fn check_kernel_stack_overflow(&self, addr: usize) -> bool;

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
