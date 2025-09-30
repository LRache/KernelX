use alloc::sync::Arc;
 
use crate::kernel::task::TCB;

pub struct TimerEvent {
    pub time: u64,
    pub tcb: Arc<TCB>,
    pub waker: Option<usize>,
}

impl PartialEq for TimerEvent {
    fn eq(&self, other: &Self) -> bool {
        self.time == other.time && Arc::ptr_eq(&self.tcb, &other.tcb)
    }
    
}

impl Eq for TimerEvent {}

impl Ord for TimerEvent {
    fn cmp(&self, other: &Self) -> core::cmp::Ordering {
        self.time.cmp(&other.time)
    }
}

impl PartialOrd for TimerEvent {
    fn partial_cmp(&self, other: &Self) -> Option<core::cmp::Ordering> {
        Some(self.cmp(other))
    }
}
