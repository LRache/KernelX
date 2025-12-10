use alloc::sync::Arc;

use crate::kernel::event::Event;
use crate::kernel::scheduler::{Task, TaskState, Tid, KernelStack, tid, current};
use crate::kernel::scheduler;
use crate::kernel::task::TCB;
use crate::klib::SpinLock;
use crate::arch::KernelContext;

pub struct KThread {
    tid: Tid,
    kcontext: KernelContext,
    kstack: KernelStack,
    state: SpinLock<TaskState>,
    wakeup_event: SpinLock<Option<Event>>,
}

impl KThread {
    fn new(tid: Tid, entry: fn()) -> Self {
        let kstack = KernelStack::new(crate::kernel::config::KTASK_KSTACK_PAGE_COUNT);
        let mut kcontext = KernelContext::new(&kstack);
        kcontext.set_entry(entry as usize);
        Self {
            tid,
            kcontext,
            kstack,
            state: SpinLock::new(TaskState::Ready),
            wakeup_event: SpinLock::new(None),
        }
    }
}

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

    fn get_kcontext_ptr(&self) -> *mut crate::arch::KernelContext {
        &self.kcontext as *const _ as *mut _
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

    fn wakeup_uninterruptible(&self, event: Event) {
        let mut state = self.state.lock();
        match *state {
            TaskState::Blocked | TaskState::BlockedUninterruptible => {},
            _ => return,
        }
        *state = TaskState::Ready;
        *self.wakeup_event.lock() = Some(event);
    }

    fn take_wakeup_event(&self) -> Option<Event> {
        self.wakeup_event.lock().take()
    }
}

pub fn spawn(entry: fn()) -> Arc<KThread> {
    let tid = tid::alloc();
    let kthread = KThread::new(tid, entry);
    let task = Arc::new(kthread);
    scheduler::push_task(task.clone());
    task
}
