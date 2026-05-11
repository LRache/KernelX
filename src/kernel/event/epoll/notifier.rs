use alloc::sync::{Arc, Weak};
use alloc::vec::Vec;

use crate::klib::SpinLock;

use super::super::FileEvent;
use super::listener::EpollListener;

pub struct EpollNotifier {
    listeners: SpinLock<Vec<Weak<EpollListener>>>,
}

impl EpollNotifier {
    pub fn new() -> Self {
        Self {
            listeners: SpinLock::new(Vec::new(), "EpollNotifier::listeners"),
        }
    }

    pub(super) fn id(&self) -> usize {
        self as *const Self as usize
    }

    pub(super) fn add_listener(&self, listener: &Arc<EpollListener>) {
        let listener_id = listener.id();
        let mut listeners = self.listeners.lock();
        listeners.retain(|weak| weak.upgrade().is_some_and(|listener| listener.id() != listener_id));
        listeners.push(Arc::downgrade(listener));
    }

    pub(super) fn remove_listener(&self, listener_id: usize) {
        self.listeners
            .lock()
            .retain(|weak| weak.upgrade().is_some_and(|listener| listener.id() != listener_id));
    }

    pub fn notify(&self, event: FileEvent) {
        if event.is_empty() {
            return;
        }

        let listeners = {
            let mut listeners = self.listeners.lock();
            let mut upgraded = Vec::new();
            listeners.retain(|weak| {
                if let Some(listener) = weak.upgrade() {
                    upgraded.push(listener);
                    true
                } else {
                    false
                }
            });
            upgraded
        };

        let id = self.id();
        listeners.iter().for_each(|listener| {
            listener.notify(id, event);
        });
    }
}
