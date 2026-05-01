use alloc::sync::{Arc, Weak};
use alloc::vec::Vec;
use bitflags::bitflags;

use crate::fs::file::FileOps;
use crate::klib::SpinLock;

bitflags! {
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub struct FanotifyEventMask: u64 {
        const FAN_ACCESS = 0x0000_0001;
        const FAN_MODIFY = 0x0000_0002;
        const FAN_CLOSE_WRITE = 0x0000_0008;
        const FAN_CLOSE_NOWRITE = 0x0000_0010;
        const FAN_OPEN = 0x0000_0020;
        const FAN_EVENT_ON_CHILD = 0x0800_0000;
        const FAN_ONDIR = 0x4000_0000;
    }
}

bitflags! {
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub struct FanotifyMarkFlags: usize {
        const FAN_MARK_ADD = 0x0000_0001;
        const FAN_MARK_REMOVE = 0x0000_0002;
        const FAN_MARK_DONT_FOLLOW = 0x0000_0004;
        const FAN_MARK_ONLYDIR = 0x0000_0008;
        const FAN_MARK_MOUNT = 0x0000_0010;
        const FAN_MARK_IGNORED_MASK = 0x0000_0020;
        const FAN_MARK_IGNORED_SURV_MODIFY = 0x0000_0040;
        const FAN_MARK_FLUSH = 0x0000_0080;
        const FAN_MARK_FILESYSTEM = 0x0000_0100;
        const FAN_MARK_EVICTABLE = 0x0000_0200;
        const FAN_MARK_IGNORE = 0x0000_0400;
    }
}

pub struct Fanotify {
    state: SpinLock<FanotifyState>,
}

pub trait FanotifyListener: Send + Sync {
    fn fanotify_id(&self) -> usize;
    fn fanotify_generation(&self) -> usize;
    fn queue_fanotify_event(&self, mask: FanotifyEventMask, file: Option<Arc<dyn FileOps>>);
}

struct FanotifyMark {
    listener_id: usize,
    generation: usize,
    listener: Weak<dyn FanotifyListener>,
    flags: FanotifyMarkFlags,
    mask: FanotifyEventMask,
}

pub struct FanotifyState {
    marks: Vec<FanotifyMark>,
}

impl Fanotify {
    pub fn new() -> Self {
        Self {
            state: SpinLock::new(FanotifyState::new(), "Fanotify::state"),
        }
    }

    pub fn state(&self) -> &SpinLock<FanotifyState> {
        &self.state
    }

    pub fn add_mark(&self, listener: &Arc<dyn FanotifyListener>, flags: FanotifyMarkFlags, mask: FanotifyEventMask) {
        self.state.lock().add_mark(listener, flags, mask);
    }

    pub fn remove_mark(&self, listener_id: usize, generation: usize, mask: FanotifyEventMask) {
        self.state.lock().remove_mark(listener_id, generation, mask);
    }

    pub fn notify(&self, mask: FanotifyEventMask, file: Option<Arc<dyn FileOps>>) {
        let listeners = {
            let mut state = self.state.lock();
            state.listeners_for_mask(mask)
        };

        for listener in listeners {
            listener.queue_fanotify_event(mask, file.clone());
        }
    }

    pub fn notify_child(&self, mask: FanotifyEventMask, file: Option<Arc<dyn FileOps>>) {
        let listeners = {
            let mut state = self.state.lock();
            state.listeners_for_child_mask(mask)
        };

        for listener in listeners {
            listener.queue_fanotify_event(mask, file.clone());
        }
    }
}

impl FanotifyState {
    pub const fn new() -> Self {
        Self { marks: Vec::new() }
    }

    fn add_mark(&mut self, listener: &Arc<dyn FanotifyListener>, flags: FanotifyMarkFlags, mask: FanotifyEventMask) {
        let listener_id = listener.fanotify_id();
        let generation = listener.fanotify_generation();
        if let Some(mark) = self
            .marks
            .iter_mut()
            .find(|mark| mark.listener_id == listener_id && mark.generation == generation)
        {
            mark.flags = flags;
            mark.mask.insert(mask);
            return;
        }

        self.marks.push(FanotifyMark {
            listener_id,
            generation,
            listener: Arc::downgrade(listener),
            flags,
            mask,
        });
    }

    fn remove_mark(&mut self, listener_id: usize, generation: usize, mask: FanotifyEventMask) {
        for mark in self
            .marks
            .iter_mut()
            .filter(|mark| mark.listener_id == listener_id && mark.generation == generation)
        {
            mark.mask.remove(mask);
        }
        self.marks.retain(|mark| !mark.mask.is_empty());
    }

    fn listeners_for_mask(&mut self, mask: FanotifyEventMask) -> Vec<Arc<dyn FanotifyListener>> {
        let mut listeners = Vec::new();
        self.marks.retain(|mark| {
            let Some(listener) = mark.listener.upgrade() else {
                return false;
            };
            if listener.fanotify_generation() != mark.generation {
                return false;
            }
            if mark.mask.intersects(mask) {
                listeners.push(listener);
            }
            true
        });
        listeners
    }

    fn listeners_for_child_mask(&mut self, mask: FanotifyEventMask) -> Vec<Arc<dyn FanotifyListener>> {
        let mut listeners = Vec::new();
        self.marks.retain(|mark| {
            let Some(listener) = mark.listener.upgrade() else {
                return false;
            };
            if listener.fanotify_generation() != mark.generation {
                return false;
            }
            if mark.mask.contains(FanotifyEventMask::FAN_EVENT_ON_CHILD) && mark.mask.intersects(mask) {
                listeners.push(listener);
            }
            true
        });
        listeners
    }
}

impl Default for Fanotify {
    fn default() -> Self {
        Self::new()
    }
}

impl Default for FanotifyState {
    fn default() -> Self {
        Self::new()
    }
}
