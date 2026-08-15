use alloc::collections::VecDeque;
use alloc::sync::Arc;

use crate::kernel::scheduler;
use crate::kernel::scheduler::{Task, current};

use super::Event;

struct WaitQueueItem<T> {
    task: Arc<dyn Task>,
    arg: T,
    wakeup: fn(Arc<dyn Task>, Event),
}

impl<T> WaitQueueItem<T> {
    fn new(task: Arc<dyn Task>, arg: T, wakeup: fn(Arc<dyn Task>, Event)) -> Self {
        Self { task, arg, wakeup }
    }
}

fn wakeup_task(task: Arc<dyn Task>, event: Event) {
    let _ = scheduler::wakeup_task(task, event);
}

fn set_pending_wakeup_if_running(task: Arc<dyn Task>, event: Event) {
    if !task.set_pending_wakeup_if_running(event.clone()) {
        let _ = scheduler::wakeup_task(task, event);
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
        self.waiters.push_back(WaitQueueItem::new(task, arg, wakeup_task));
    }

    pub fn wait_pending(&mut self, task: Arc<dyn Task>, arg: T) {
        self.waiters
            .push_back(WaitQueueItem::new(task, arg, set_pending_wakeup_if_running));
    }

    pub fn wait_current(&mut self, arg: T) {
        let current = current::task();
        current.block(self.name);
        self.wait(current.clone(), arg);
    }

    pub fn wake_all(&mut self, map_arg_to_event: impl Fn(T) -> Event) {
        self.waiters.drain(..).for_each(|item| {
            (item.wakeup)(item.task, map_arg_to_event(item.arg));
        });
    }

    pub fn wake_all_by(&mut self, mut predicate: impl FnMut(&T) -> bool, map_arg_to_event: impl Fn(T) -> Event) {
        let mut i = 0;
        while i < self.waiters.len() {
            if predicate(&self.waiters[i].arg) {
                if let Some(item) = self.waiters.remove(i) {
                    (item.wakeup)(item.task, map_arg_to_event(item.arg));
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
