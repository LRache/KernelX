use crate::arch;
use crate::kernel::event::Event;
use crate::kernel::task::{TCB, Tid};

// #[derive(PartialEq, Eq, Debug)]
// pub enum TaskState {
//     Ready,
//     Running,
//     Blocked,
//     BlockedUninterruptible,
//     Exited,
// }

// pub struct Task {
//     pub tid: Tid,
//     kcontext: arch::KernelContext,
//     _kstack: KernelStack,

//     state: SpinLock<TaskState>,
//     wakeup_event: SpinLock<Option<Event>>,
    
//     meta: TaskMeta,
// }

// impl Task {
//     pub fn new_user(tid: Tid, tcb: Arc<TCB>, kstack: KernelStack) -> Self {
//         let kcontext = arch::KernelContext::new(&kstack);
//         Self {
//             tid,
//             kcontext,
//             _kstack: kstack,
            
//             state: SpinLock::new(TaskState::Ready),
//             wakeup_event: SpinLock::new(None),
            
//             meta: TaskType::User(tcb),
//         }
//     }

//     pub fn new_kernel(tid: Tid, entry: usize) -> Self {
//         let kstack = KernelStack::new(config::KTASK_KSTACK_PAGE_COUNT);
//         let mut kcontext = arch::KernelContext::new(&kstack);
//         kcontext.set_entry(entry);
//         Self {
//             tid,
//             kcontext,
//             _kstack: kstack,
            
//             state: SpinLock::new(TaskState::Ready),
//             wakeup_event: SpinLock::new(None),
            
//             meta: TaskType::Kernel,
//         }
//     }

//     pub fn tcb(&self) -> &Arc<TCB> {
//         match &self.meta {
//             TaskType::User(tcb) => tcb,
//             TaskType::Kernel => panic!("Kernel task has no TCB"),
//         }
//     }

//     pub fn get_kcontext_ptr(&self) -> *mut arch::KernelContext {
//         &self.kcontext as *const _ as *mut _
//     }

//     pub fn run(&self) {
//         *self.state.lock() = TaskState::Running;
//     }

//     pub fn set_to_ready(&self) -> bool {
//         let mut state = self.state.lock();
//         if *state == TaskState::Running {
//             *state = TaskState::Ready;
//             true
//         } else {
//             false
//         }
//     }

//     pub fn wakeup(self: &Arc<Self>, event: Event) {
//         let mut state = self.state.lock();
//         let mut wakeup_event = self.wakeup_event.lock();
//         if *state == TaskState::Blocked {
//             *state = TaskState::Ready;
//             *wakeup_event = Some(event);
//         }

//         scheduler::push_task(self.clone());
//     }

//     pub fn wakeup_uninterruptible(self: &Arc<Self>, event: Event) {
//         let mut state = self.state.lock();
//         let mut wakeup_event = self.wakeup_event.lock();
//         if *state == TaskState::BlockedUninterruptible {
//             *state = TaskState::Ready;
//             *wakeup_event = Some(event);
//         }

//         scheduler::push_task(self.clone());
//     }

//     pub fn take_wakeup_event(&self) -> Option<Event> {
//         let mut wakeup_event = self.wakeup_event.lock();
//         wakeup_event.take()
//     }

//     pub fn block(&self, _reason: &str) -> bool {
//         let mut state = self.state.lock();
//         match *state {
//             TaskState::Ready | TaskState::Running => {},
//             _ => return false,
//         }
//         *state = TaskState::Blocked;
//         return true;
//     }

//     pub fn block_uninterruptible(&self, _reason: &str) -> bool {
//         let mut state = self.state.lock();
//         match *state {
//             TaskState::Ready | TaskState::Running => {},
//             _ => return false,
//         }
//         *state = TaskState::BlockedUninterruptible;
//         return true;
//     }
// }

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
