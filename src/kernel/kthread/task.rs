use alloc::boxed::Box;
use alloc::sync::Arc;
use core::cell::UnsafeCell;

use crate::arch::KernelContext;
use crate::kernel::event::Event;
use crate::kernel::scheduler::{self, KernelStack, Task, Tid, WakeupFailure, current, tid};
use crate::kernel::task::TCB;
use crate::kernel::uapi::Uid;
use crate::klib::SpinLock;

/// All kthreads enter through this trampoline. `ptr` arrives in `a0`
/// (restored by `asm_kernel_switch`) and is a thin pointer to a
/// `Box<dyn FnOnce() + Send>` allocated during spawn. After the closure
/// returns we automatically exit so callers never need `exit_current()`.
#[inline(never)]
fn kthread_trampoline(ptr: usize) {
    let f = unsafe { Box::from_raw(ptr as *mut Box<dyn FnOnce() + Send>) };
    f();
    exit_current();
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum KThreadState {
    Running,
    Ready,
    Blocked,
    BlockedUninterruptible,
    Exited,
}

pub struct KThread {
    tid: Tid,
    kcontext: UnsafeCell<KernelContext>,
    kstack: KernelStack,
    state: SpinLock<KThreadState>,
    wakeup_event: SpinLock<Option<Event>>,
    #[cfg(feature = "lockdep")]
    lockstate: crate::klib::ksync::LockState,
}

impl KThread {
    fn new<F: FnOnce() + Send + 'static>(tid: Tid, entry: F) -> Self {
        let boxed: Box<dyn FnOnce() + Send> = Box::new(entry);
        let ptr = Box::into_raw(Box::new(boxed)) as usize;

        let kstack = KernelStack::new(crate::kernel::config::KTASK_KSTACK_PAGE_COUNT);
        let mut kcontext = KernelContext::new(&kstack);
        kcontext
            .set_entry(kthread_trampoline as *const () as usize)
            .set_arg0(ptr);
        Self {
            tid,
            kcontext: UnsafeCell::new(kcontext),
            kstack,
            state: SpinLock::new(KThreadState::Ready, "KThread::state"),
            wakeup_event: SpinLock::new(None, "KThread::wakeup_event"),
            #[cfg(feature = "lockdep")]
            lockstate: crate::klib::ksync::LockState::new(),
        }
    }
}

unsafe impl Send for KThread {}
unsafe impl Sync for KThread {}

impl Task for KThread {
    fn tid(&self) -> Tid {
        self.tid
    }

    fn euid(&self) -> Uid {
        0
    }

    fn egid(&self) -> Uid {
        0
    }

    fn fsuid(&self) -> Uid {
        0
    }

    fn fsgid(&self) -> Uid {
        0
    }

    fn tcb(&self) -> &TCB {
        unreachable!("KThread is not a TCB")
    }

    fn pause_system_time(&self) {}

    fn resume_system_time(&self) {}

    fn kstack(&self) -> &KernelStack {
        &self.kstack
    }

    fn kcontext_ptr(&self) -> *mut KernelContext {
        self.kcontext.get()
    }

    fn run_if_ready(&self) -> bool {
        let mut state = self.state.lock();
        if *state != KThreadState::Ready {
            return false;
        }
        *state = KThreadState::Running;
        true
    }

    fn state_running_to_ready(&self) -> bool {
        let mut state = self.state.lock();
        if *state != KThreadState::Running {
            return false;
        }
        *state = KThreadState::Ready;
        true
    }

    fn block(&self, _reason: &str) -> bool {
        debug_assert!(current::tid() == self.tid);
        let mut state = self.state.lock();
        match *state {
            KThreadState::Ready | KThreadState::Running => {}
            _ => return false,
        }
        *state = KThreadState::Blocked;
        true
    }

    fn block_uninterruptible(&self, _reason: &str) -> bool {
        debug_assert!(current::tid() == self.tid);
        let mut state = self.state.lock();
        match *state {
            KThreadState::Ready | KThreadState::Running => {}
            _ => return false,
        }
        *state = KThreadState::BlockedUninterruptible;
        true
    }

    fn unblock(&self) {
        let mut state = self.state.lock();
        debug_assert!(matches!(
            *state,
            KThreadState::Blocked | KThreadState::BlockedUninterruptible
        ));
        *state = KThreadState::Ready;
    }

    fn wakeup(&self, event: Event) -> Result<(), WakeupFailure> {
        let mut state = self.state.lock();
        match *state {
            KThreadState::Blocked => {
                *state = KThreadState::Ready;
                *self.wakeup_event.lock() = Some(event);
                Ok(())
            }
            KThreadState::BlockedUninterruptible => Err(WakeupFailure::BlockedUninterruptible),
            _ => Err(WakeupFailure::NotBlocked),
        }
    }

    fn wakeup_uninterruptible(&self, event: Event) -> bool {
        let mut state = self.state.lock();
        match *state {
            KThreadState::Blocked | KThreadState::BlockedUninterruptible => {}
            _ => return false,
        }
        *state = KThreadState::Ready;
        *self.wakeup_event.lock() = Some(event);
        true
    }

    fn take_wakeup_event(&self) -> Option<Event> {
        self.wakeup_event.lock().take()
    }

    fn set_exited(&self) {
        *self.state.lock() = KThreadState::Exited;
    }

    #[cfg(feature = "lockdep")]
    fn lockstate(&self) -> &crate::klib::ksync::LockState {
        &self.lockstate
    }
}

pub fn spawn<F: FnOnce() + Send + 'static>(entry: F) -> Arc<KThread> {
    let tid = tid::alloc();
    let kthread = KThread::new(tid, entry);
    let task = Arc::new(kthread);
    scheduler::push_task(task.clone());
    task
}

pub fn exit_current() -> ! {
    current::task().set_exited();
    current::schedule();
    unreachable!()
}
