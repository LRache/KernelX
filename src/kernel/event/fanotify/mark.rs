use alloc::sync::{Arc, Weak};
use alloc::vec::Vec;

use crate::fs::file::FileOps;
use crate::fs::inode::FileType;
use crate::fs::Dentry;
use crate::klib::SpinLock;

use super::file::FanotifyListener;
use super::types::{FanotifyEventMask, FanotifyFdinfoKey, FanotifyMarkFlags};

pub struct Fanotify {
    state: SpinLock<FanotifyState>,
}

#[derive(Clone, Copy)]
pub struct FanotifyEventContext {
    pub is_dir: bool,
}

pub struct FanotifyListenerMask {
    pub listener: Arc<FanotifyListener>,
    pub mark_mask: FanotifyEventMask,
    pub ignored_mask: FanotifyEventMask,
}

struct FanotifyMaskSet {
    self_mask: FanotifyEventMask,
    child_mask: FanotifyEventMask,
}

struct FanotifyMark {
    listener_id: usize,
    generation: usize,
    listener: Weak<FanotifyListener>,
    watched: FanotifyMaskSet,
    ignored: FanotifyMaskSet,
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

    pub fn add_mark(
        &self,
        listener: &Arc<FanotifyListener>,
        flags: FanotifyMarkFlags,
        mask: FanotifyEventMask,
        fdinfo_key: FanotifyFdinfoKey,
    ) {
        self.state.lock().add_mark(listener, flags, mask, fdinfo_key);
    }

    pub fn remove_mark(
        &self,
        listener_id: usize,
        generation: usize,
        flags: FanotifyMarkFlags,
        mask: FanotifyEventMask,
        fdinfo_key: FanotifyFdinfoKey,
    ) {
        self.state
            .lock()
            .remove_mark(listener_id, generation, flags, mask, fdinfo_key);
    }

    pub fn listener_masks(
        &self,
        mask: FanotifyEventMask,
        context: FanotifyEventContext,
        child_event: bool,
    ) -> Vec<FanotifyListenerMask> {
        self.state.lock().listener_masks(mask, context, child_event)
    }

    pub fn dentry_context(dentry: &Arc<Dentry>) -> FanotifyEventContext {
        FanotifyEventContext {
            is_dir: matches!(dentry.get_inode().inode_type(), Ok(FileType::Directory)),
        }
    }

    pub fn file_context(file: Option<&Arc<dyn FileOps>>) -> FanotifyEventContext {
        let dentry = file.and_then(|file| file.get_dentry());
        FanotifyEventContext {
            is_dir: dentry
                .map(|dentry| matches!(dentry.get_inode().inode_type(), Ok(FileType::Directory)))
                .unwrap_or(false),
        }
    }
}

impl FanotifyState {
    pub const fn new() -> Self {
        Self { marks: Vec::new() }
    }

    fn add_mark(
        &mut self,
        listener: &Arc<FanotifyListener>,
        flags: FanotifyMarkFlags,
        mask: FanotifyEventMask,
        fdinfo_key: FanotifyFdinfoKey,
    ) {
        let listener_id = listener.fanotify_id();
        let generation = listener.fanotify_generation();
        let ignore_mark = flags.is_ignore_mark();
        if let Some(mark) = self
            .marks
            .iter_mut()
            .find(|mark| mark.listener_id == listener_id && mark.generation == generation)
        {
            if ignore_mark {
                mark.ignored.insert(mask);
            } else {
                mark.watched.insert(mask);
            }
            listener.add_fanotify_mark(fdinfo_key, flags, mask);
            return;
        }

        let mut watched = FanotifyMaskSet::new();
        let mut ignored = FanotifyMaskSet::new();
        if ignore_mark {
            ignored.insert(mask);
        } else {
            watched.insert(mask);
        }
        self.marks.push(FanotifyMark {
            listener_id,
            generation,
            listener: Arc::downgrade(listener),
            watched,
            ignored,
        });
        listener.add_fanotify_mark(fdinfo_key, flags, mask);
    }

    fn remove_mark(
        &mut self,
        listener_id: usize,
        generation: usize,
        flags: FanotifyMarkFlags,
        mask: FanotifyEventMask,
        fdinfo_key: FanotifyFdinfoKey,
    ) {
        let ignore_mark = flags.is_ignore_mark();
        for mark in self
            .marks
            .iter_mut()
            .filter(|mark| mark.listener_id == listener_id && mark.generation == generation)
        {
            if let Some(listener) = mark.listener.upgrade() {
                listener.remove_fanotify_mark(fdinfo_key, flags, mask);
            }
            if ignore_mark {
                mark.ignored.remove(mask);
            } else {
                mark.watched.remove(mask);
            }
        }
        self.marks
            .retain(|mark| !mark.watched.is_empty() || !mark.ignored.is_empty());
    }

    fn listener_masks(
        &mut self,
        mask: FanotifyEventMask,
        context: FanotifyEventContext,
        child_event: bool,
    ) -> Vec<FanotifyListenerMask> {
        let mut listener_masks = Vec::new();
        self.marks.retain(|mark| {
            let Some(listener) = mark.listener.upgrade() else {
                return false;
            };
            if listener.fanotify_generation() != mark.generation {
                return false;
            }
            let mark_mask = mark.watched.matching(mask, context, child_event);
            let ignored_mask = mark.ignored.matching(mask, context, child_event);
            if !mark_mask.is_empty() || !ignored_mask.is_empty() {
                listener_masks.push(FanotifyListenerMask {
                    listener,
                    mark_mask,
                    ignored_mask,
                });
            }
            true
        });
        listener_masks
    }
}

impl FanotifyMaskSet {
    const fn new() -> Self {
        Self {
            self_mask: FanotifyEventMask::empty(),
            child_mask: FanotifyEventMask::empty(),
        }
    }

    fn is_empty(&self) -> bool {
        self.self_mask.is_empty() && self.child_mask.is_empty()
    }

    fn insert(&mut self, mask: FanotifyEventMask) {
        let event_mask = Self::event_mask(mask);
        let child_enabled = !self.child_mask.is_empty() || mask.contains(FanotifyEventMask::FAN_EVENT_ON_CHILD);

        self.self_mask.insert(event_mask);
        if child_enabled {
            self.child_mask.insert(self.self_mask);
        }
    }

    fn remove(&mut self, mask: FanotifyEventMask) {
        let event_mask = Self::event_mask(mask);

        self.self_mask.remove(event_mask);
        self.child_mask.remove(event_mask);
        if mask.contains(FanotifyEventMask::FAN_EVENT_ON_CHILD) {
            self.child_mask = FanotifyEventMask::empty();
        }
    }

    fn matching(&self, mask: FanotifyEventMask, context: FanotifyEventContext, child_event: bool) -> FanotifyEventMask {
        let mark_mask = if child_event { self.child_mask } else { self.self_mask };
        Self::matching_event_mask(mark_mask, context, mask)
    }

    fn event_mask(mask: FanotifyEventMask) -> FanotifyEventMask {
        let mut event_mask = mask;
        event_mask.remove(FanotifyEventMask::FAN_EVENT_ON_CHILD);
        event_mask
    }

    fn matching_event_mask(
        mark_mask: FanotifyEventMask,
        context: FanotifyEventContext,
        event_mask: FanotifyEventMask,
    ) -> FanotifyEventMask {
        if context.is_dir && !mark_mask.contains(FanotifyEventMask::FAN_ONDIR) {
            return FanotifyEventMask::empty();
        }
        if !mark_mask.event_bits().intersects(event_mask.event_bits()) {
            return FanotifyEventMask::empty();
        }
        mark_mask
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
