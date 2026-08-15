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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WakeupAction {
    /// The task is no longer running on a CPU and can be enqueued now.
    Enqueue,

    /// The task is still running on a CPU. Its switch tail will enqueue it.
    Deferred,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TaskRunState {
    Ready,
    Running,
    Blocked,
    Other,
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
    // PERF_DEBUG(scheduler-time): cpu_id/now_us are temporary switch-out
    // accounting inputs.
    fn finish_switch(&self, cpu_id: usize, now_us: u64) -> bool;

    // PERF_DEBUG(scheduler-time): name is retained only for temporary
    // blocked-time attribution.
    fn block(&self, name: &'static str) -> bool;
    fn block_uninterruptible(&self, name: &'static str) -> bool;
    fn unblock(&self);

    fn wakeup(&self, event: Event) -> Result<WakeupAction, WakeupFailure>;
    fn wakeup_uninterruptible(&self, event: Event) -> Option<WakeupAction>;
    fn set_pending_wakeup_if_running(&self, _event: Event) -> bool {
        false
    }
    fn take_wakeup_event(&self) -> Option<Event>;

    fn pause_system_time(&self) {}
    fn resume_system_time(&self) {}

    fn is_user_task(&self) -> bool {
        false
    }

    fn tcb(&self) -> &TCB;

    fn set_exited(&self) {}

    #[cfg(feature = "lockdep")]
    fn lockstate(&self) -> &crate::klib::ksync::LockState;
}
