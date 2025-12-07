use alloc::boxed::Box;

pub struct TimerEvent {
    pub time: u64,
    // pub task: Arc<dyn Task>,
    pub callback: Box<dyn FnOnce()>,
    pub id: u64,
}

impl PartialEq for TimerEvent {
    fn eq(&self, other: &Self) -> bool {
        // self.time == other.time && Arc::ptr_eq(&self.task, &other.task)
        self.time == other.time
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
