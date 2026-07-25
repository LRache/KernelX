use alloc::collections::VecDeque;
use alloc::sync::Arc;

use crate::kernel::scheduler;
use crate::kernel::scheduler::{Task, current};

use super::Event;

struct WaitQueueItem<T> {
    task: Arc<dyn Task>,
    arg: T,
}

impl<T> WaitQueueItem<T> {
    fn new(task: Arc<dyn Task>, arg: T) -> Self {
        Self { task, arg }
    }
}

pub struct WaitQueue<T> {
    waiters: VecDeque<WaitQueueItem<T>>,
    name: &'static str,
}

impl<T> WaitQueue<T> {
    pub fn new(name: &'static str) -> Self {
        Self {
            waiters: VecDeque::new(),
            name,
        }
    }

    pub fn wait(&mut self, task: Arc<dyn Task>, arg: T) {
        self.waiters.push_back(WaitQueueItem::new(task, arg));
    }

    pub fn wait_current(&mut self, arg: T) {
        let current = current::task();
        current.block(self.name);
        self.wait(current.clone(), arg);
    }

    pub fn wake_all(&mut self, map_arg_to_event: impl Fn(T) -> Event) {
        self.waiters.drain(..).for_each(|item| {
            let _ = scheduler::wakeup_task(item.task, map_arg_to_event(item.arg));
        });
    }

    pub fn wake_all_by(&mut self, mut predicate: impl FnMut(&T) -> bool, map_arg_to_event: impl Fn(T) -> Event) {
        let mut i = 0;
        while i < self.waiters.len() {
            if predicate(&self.waiters[i].arg) {
                if let Some(item) = self.waiters.remove(i) {
                    let _ = scheduler::wakeup_task(item.task, map_arg_to_event(item.arg));
                }
            } else {
                i += 1;
            }
        }
    }

    pub fn count_by(&self, mut predicate: impl FnMut(&T) -> bool) -> usize {
        let mut count = 0;
        for item in &self.waiters {
            if predicate(&item.arg) {
                count += 1;
            }
        }
        count
    }

    pub fn remove(&mut self, task: &Arc<dyn Task>) {
        self.waiters.retain(|item| !Arc::ptr_eq(&item.task, task));
    }

    pub fn remove_current(&mut self) {
        self.remove(current::task());
    }
}
