use alloc::boxed::Box;
use alloc::sync::Arc;
use core::cell::UnsafeCell;

use crate::arch::KernelContext;
use crate::kernel::config::KTASK_KSTACK_PAGE_COUNT;
use crate::kernel::event::Event;
#[cfg(feature = "watchdog")]
use crate::kernel::scheduler::TaskRunState;
use crate::kernel::scheduler::{self, KernelStack, Task, Tid, WakeupAction, WakeupFailure, current, tid};
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

#[cfg(feature = "watchdog")]
fn task_run_state(state: KThreadState) -> TaskRunState {
    match state {
        KThreadState::Ready => TaskRunState::Ready,
        KThreadState::Running => TaskRunState::Running,
        KThreadState::Blocked | KThreadState::BlockedUninterruptible => TaskRunState::Blocked,
        KThreadState::Exited => TaskRunState::Other,
    }
}

struct KThreadStateSet {
    state: KThreadState,
    on_cpu: bool,
    wake_pending: bool,
    #[cfg(feature = "watchdog")]
    watchdog_counted: bool,
}

impl KThreadStateSet {
    fn set_state(&mut self, state: KThreadState) {
        #[cfg(feature = "watchdog")]
        if self.watchdog_counted {
            scheduler::watchdog::transition_task(task_run_state(self.state), task_run_state(state));
        }
        self.state = state;
    }
}

pub struct KThread {
    tid: Tid,
    kcontext: UnsafeCell<KernelContext>,
    kstack: KernelStack<{ KTASK_KSTACK_PAGE_COUNT - 1 }>,
    state: SpinLock<KThreadStateSet>,
    wakeup_event: SpinLock<Option<Event>>,
    #[cfg(feature = "lockdep")]
    lockstate: crate::klib::ksync::LockState,
}

impl KThread {
    fn new<F: FnOnce() + Send + 'static>(tid: Tid, entry: F) -> Self {
        let boxed: Box<dyn FnOnce() + Send> = Box::new(entry);
        let ptr = Box::into_raw(Box::new(boxed)) as usize;

        let kstack = KernelStack::new();
        let mut kcontext = KernelContext::new(&kstack);
        kcontext
            .set_entry(kthread_trampoline as *const () as usize)
            .set_arg0(ptr);
        Self {
            tid,
            kcontext: UnsafeCell::new(kcontext),
            kstack,
            state: SpinLock::new(
                KThreadStateSet {
                    state: KThreadState::Ready,
                    on_cpu: false,
                    wake_pending: false,
                    #[cfg(feature = "watchdog")]
                    watchdog_counted: false,
                },
                "KThread::state",
            ),
            wakeup_event: SpinLock::new(None, "KThread::wakeup_event"),
            #[cfg(feature = "lockdep")]
            lockstate: crate::klib::ksync::LockState::new(),
        }
    }

    #[cfg(feature = "watchdog")]
    fn register_watchdog(&self) {
        let mut state = self.state.lock();
        if state.watchdog_counted {
            return;
        }
        scheduler::watchdog::register_task(task_run_state(state.state));
        state.watchdog_counted = true;
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

    fn check_kernel_stack_overflow(&self, addr: usize) -> bool {
        self.kstack.check_stack_overflow(addr)
    }

    fn kcontext_ptr(&self) -> *mut KernelContext {
        self.kcontext.get()
    }

    fn run_if_ready(&self) -> bool {
        let mut state = self.state.lock();
        if state.state != KThreadState::Ready {
            return false;
        }
        debug_assert!(!state.on_cpu);
        debug_assert!(!state.wake_pending);
        state.set_state(KThreadState::Running);
        state.on_cpu = true;
        true
    }

    fn finish_switch(&self) -> bool {
        let mut state = self.state.lock();
        debug_assert!(state.on_cpu);
        state.on_cpu = false;

        if state.wake_pending {
            state.wake_pending = false;
            if matches!(
                state.state,
                KThreadState::Blocked | KThreadState::BlockedUninterruptible
            ) {
                state.set_state(KThreadState::Ready);
                return true;
            }
        }

        if state.state == KThreadState::Running {
            state.set_state(KThreadState::Ready);
            return true;
        }
        false
    }

    fn block(&self, _reason: &str) -> bool {
        debug_assert!(current::tid() == self.tid);
        let mut state = self.state.lock();
        match state.state {
            KThreadState::Ready | KThreadState::Running => {}
            _ => return false,
        }
        debug_assert!(state.on_cpu);
        debug_assert!(!state.wake_pending);
        state.set_state(KThreadState::Blocked);
        true
    }

    fn block_uninterruptible(&self, _reason: &str) -> bool {
        debug_assert!(current::tid() == self.tid);
        let mut state = self.state.lock();
        match state.state {
            KThreadState::Ready | KThreadState::Running => {}
            _ => return false,
        }
        debug_assert!(state.on_cpu);
        debug_assert!(!state.wake_pending);
        state.set_state(KThreadState::BlockedUninterruptible);
        true
    }

    fn unblock(&self) {
        let mut state = self.state.lock();
        debug_assert!(state.on_cpu);
        debug_assert!(matches!(
            state.state,
            KThreadState::Blocked | KThreadState::BlockedUninterruptible
        ));
        state.set_state(KThreadState::Running);
        state.wake_pending = false;
    }

    fn cancel_block(&self) {
        let mut state = self.state.lock();
        debug_assert!(state.on_cpu);
        if matches!(
            state.state,
            KThreadState::Blocked | KThreadState::BlockedUninterruptible
        ) {
            state.set_state(KThreadState::Running);
        }
        state.wake_pending = false;
        drop(state);
        self.wakeup_event.lock().take();
    }

    fn wakeup(&self, event: Event) -> Result<WakeupAction, WakeupFailure> {
        let mut state = self.state.lock();
        if state.wake_pending {
            return Err(WakeupFailure::NotBlocked);
        }
        match state.state {
            KThreadState::Blocked => {
                *self.wakeup_event.lock() = Some(event);
                if state.on_cpu {
                    state.wake_pending = true;
                    Ok(WakeupAction::Deferred)
                } else {
                    state.set_state(KThreadState::Ready);
                    Ok(WakeupAction::Enqueue)
                }
            }
            KThreadState::BlockedUninterruptible => Err(WakeupFailure::BlockedUninterruptible),
            _ => Err(WakeupFailure::NotBlocked),
        }
    }

    fn wakeup_uninterruptible(&self, event: Event) -> Option<WakeupAction> {
        let mut state = self.state.lock();
        if state.wake_pending {
            return None;
        }
        match state.state {
            KThreadState::Blocked | KThreadState::BlockedUninterruptible => {}
            _ => return None,
        }
        *self.wakeup_event.lock() = Some(event);
        if state.on_cpu {
            state.wake_pending = true;
            Some(WakeupAction::Deferred)
        } else {
            state.set_state(KThreadState::Ready);
            Some(WakeupAction::Enqueue)
        }
    }

    fn take_wakeup_event(&self) -> Option<Event> {
        self.wakeup_event.lock().take()
    }

    fn set_exited(&self) {
        let mut state = self.state.lock();
        state.set_state(KThreadState::Exited);
        state.wake_pending = false;
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
    #[cfg(feature = "watchdog")]
    task.register_watchdog();
    scheduler::push_task(task.clone());
    task
}

pub fn exit_current() -> ! {
    current::task().set_exited();
    current::schedule();
    unreachable!()
}
