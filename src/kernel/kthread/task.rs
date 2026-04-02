use core::cell::UnsafeCell;

use alloc::sync::Arc;
use alloc::boxed::Box;

use crate::kernel::event::Event;
use crate::kernel::scheduler::{Task, TaskState, Tid, KernelStack, tid, current};
use crate::kernel::scheduler;
use crate::kernel::task::TCB;
use crate::klib::SpinLock;
use crate::arch::KernelContext;

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

pub struct KThread {
    tid: Tid,
    kcontext: UnsafeCell<KernelContext>,
    kstack: KernelStack,
    state: SpinLock<TaskState>,
    wakeup_event: SpinLock<Option<Event>>,
    #[cfg(feature = "deadlock-detect")]
    lockstate: crate::klib::ksync::LockState,
}

impl KThread {
    fn new<F: FnOnce() + Send + 'static>(tid: Tid, entry: F) -> Self {
        let boxed: Box<dyn FnOnce() + Send> = Box::new(entry);
        let ptr = Box::into_raw(Box::new(boxed)) as usize;

        let kstack = KernelStack::new(crate::kernel::config::KTASK_KSTACK_PAGE_COUNT);
        let mut kcontext = KernelContext::new(&kstack);
        kcontext.set_entry(kthread_trampoline as usize).set_arg0(ptr);
        Self {
            tid,
            kcontext: UnsafeCell::new(kcontext),
            kstack,
            state: SpinLock::new(TaskState::Ready, "KThread::state"),
            wakeup_event: SpinLock::new(None, "KThread::wakeup_event"),
            #[cfg(feature = "deadlock-detect")]
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

    fn tcb(&self) -> &TCB {
        unreachable!("KThread is not a TCB")
    }

    fn kstack(&self) -> &KernelStack {
        &self.kstack
    }

    fn kcontext(&self) -> &mut KernelContext {
        unsafe { self.kcontext.get().as_mut() }.unwrap()
    }

    fn run_if_ready(&self) -> bool {
        let mut state = self.state.lock();
        if *state != TaskState::Ready {
            return false;
        }
        *state = TaskState::Running;
        true
    }

    fn state_running_to_ready(&self) -> bool {
        let mut state = self.state.lock();
        if *state != TaskState::Running {
            return false;
        }
        *state = TaskState::Ready;
        true
    }

    fn block(&self, _reason: &str) -> bool {
        debug_assert!(current::tid() == self.tid);
        let mut state = self.state.lock();
        match *state {
            TaskState::Ready | TaskState::Running => {},
            _ => return false,
        }
        *state = TaskState::Blocked;
        true
    }

    fn block_uninterruptible(&self, _reason: &str) -> bool {
        debug_assert!(current::tid() == self.tid);
        let mut state = self.state.lock();
        match *state {
            TaskState::Ready | TaskState::Running => {},
            _ => return false,
        }
        *state = TaskState::BlockedUninterruptible;
        true
    }

    fn unblock(&self) {
        let mut state = self.state.lock();
        debug_assert!(matches!(*state, TaskState::Blocked | TaskState::BlockedUninterruptible));
        *state = TaskState::Ready;
    }

    fn wakeup(&self, event: Event) -> bool {
        let mut state = self.state.lock();
        if *state != TaskState::Blocked {
            return false;
        }
        *state = TaskState::Ready;
        *self.wakeup_event.lock() = Some(event);
        true
    }

    fn wakeup_uninterruptible(&self, event: Event) -> bool {
        let mut state = self.state.lock();
        match *state {
            TaskState::Blocked | TaskState::BlockedUninterruptible => {},
            _ => return false,
        }
        *state = TaskState::Ready;
        *self.wakeup_event.lock() = Some(event);
        true
    }

    fn take_wakeup_event(&self) -> Option<Event> {
        self.wakeup_event.lock().take()
    }

    fn set_exited(&self) {
        *self.state.lock() = TaskState::Exited;
    }

    #[cfg(feature = "deadlock-detect")]
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
