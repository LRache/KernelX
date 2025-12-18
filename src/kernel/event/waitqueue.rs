use alloc::sync::Arc;
use alloc::collections::VecDeque;

use crate::kernel::scheduler;
use crate::kernel::scheduler::current;
use crate::kernel::scheduler::Task;

use super::Event;

struct WaitQueueItem<T: Copy> {
    task: Arc<dyn Task>,
    arg:  T,
}

pub struct WaitQueue<T: Copy> {
    waiters: VecDeque<WaitQueueItem<T>>,
}

impl<T: Copy> WaitQueueItem<T> {
    fn new(task: Arc<dyn Task>, arg: T) -> Self {
        Self { task, arg }
    }

    fn wakeup(self, e: Event) {
        scheduler::wakeup_task(self.task, e);
    }
}

impl<T: Copy> WaitQueue<T> {
    pub fn new() -> Self {
        Self {
            waiters: VecDeque::new(),
        }
    }

    pub fn wait(&mut self, task: Arc<dyn Task>, arg: T) {
        self.waiters.push_back(WaitQueueItem::new(task, arg));
    }

    pub fn wait_current(&mut self, arg: T) {
        let current = current::task();
        current.block("waitqueue");
        self.wait(current.clone(), arg);
    }

    pub fn wake_all(&mut self, map_arg_to_event: impl Fn(T) -> Event) {
        self.waiters.drain(..).for_each(|item| {
            let arg = item.arg;
            item.wakeup(map_arg_to_event(arg));
        });
    }

    pub fn remove(&mut self, task: &Arc<dyn Task>) {
        if let Some(pos) = self.waiters.iter().position(|item| Arc::ptr_eq(&item.task, task)) {
            self.waiters.remove(pos);
        }
    }
}
