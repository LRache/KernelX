use alloc::sync::{Arc, Weak};
use alloc::vec::Vec;

use crate::fs::Dentry;
use crate::fs::file::FileOps;
use crate::fs::inode::FileType;
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

struct FanotifyIgnoredFdinfoMark {
    key: FanotifyFdinfoKey,
    flags: FanotifyMarkFlags,
    mask: FanotifyEventMask,
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
    ignored_surv_modify: FanotifyMaskSet,
    ignored_fdinfo: Vec<FanotifyIgnoredFdinfoMark>,
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
            is_dir: matches!(
                dentry.get_inode().and_then(|inode| inode.inode_type()),
                Ok(FileType::Directory)
            ),
        }
    }

    pub fn file_context(file: Option<&Arc<dyn FileOps>>) -> FanotifyEventContext {
        let dentry = file.and_then(|file| file.get_dentry());
        FanotifyEventContext {
            is_dir: dentry
                .map(|dentry| {
                    matches!(
                        dentry.get_inode().and_then(|inode| inode.inode_type()),
                        Ok(FileType::Directory)
                    )
                })
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
                mark.insert_ignored(flags, mask, fdinfo_key);
            } else {
                mark.watched.insert(mask);
            }
            listener.add_fanotify_mark(fdinfo_key, flags, mask);
            return;
        }

        let mut watched = FanotifyMaskSet::new();
        if !ignore_mark {
            watched.insert(mask);
        }
        let mut mark = FanotifyMark {
            listener_id,
            generation,
            listener: Arc::downgrade(listener),
            watched,
            ignored: FanotifyMaskSet::new(),
            ignored_surv_modify: FanotifyMaskSet::new(),
            ignored_fdinfo: Vec::new(),
        };
        if ignore_mark {
            mark.insert_ignored(flags, mask, fdinfo_key);
        }
        self.marks.push(mark);
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
                mark.remove_ignored(mask);
            } else {
                mark.watched.remove(mask);
            }
        }
        self.marks.retain(|mark| !mark.is_empty());
    }

    fn listener_masks(
        &mut self,
        mask: FanotifyEventMask,
        context: FanotifyEventContext,
        child_event: bool,
    ) -> Vec<FanotifyListenerMask> {
        let mut listener_masks = Vec::new();
        let mut index = 0;
        while index < self.marks.len() {
            let mark = &mut self.marks[index];
            let Some(listener) = mark.listener.upgrade() else {
                self.marks.remove(index);
                continue;
            };
            if listener.fanotify_generation() != mark.generation {
                self.marks.remove(index);
                continue;
            }
            let mark_mask = mark.watched.matching(mask, context, child_event);
            let mut ignored_mask = mark.ignored.matching(mask, context, child_event);
            ignored_mask.insert(mark.ignored_surv_modify.matching(mask, context, child_event));
            if !mark_mask.is_empty() || !ignored_mask.is_empty() {
                listener_masks.push(FanotifyListenerMask {
                    listener: listener.clone(),
                    mark_mask,
                    ignored_mask,
                });
            }
            if mask.intersects(FanotifyEventMask::FAN_MODIFY) && mark.ignored.applies_to(context, child_event) {
                mark.clear_ignored_after_modify(&listener);
            }
            if mark.is_empty() {
                self.marks.remove(index);
            } else {
                index += 1;
            }
        }
        listener_masks
    }
}

impl FanotifyMark {
    fn insert_ignored(&mut self, flags: FanotifyMarkFlags, mask: FanotifyEventMask, fdinfo_key: FanotifyFdinfoKey) {
        if flags.contains(FanotifyMarkFlags::FAN_MARK_IGNORED_SURV_MODIFY) {
            self.ignored_surv_modify.insert(mask);
            return;
        }

        self.ignored.insert(mask);
        if let Some(mark) = self
            .ignored_fdinfo
            .iter_mut()
            .find(|mark| mark.key == fdinfo_key && mark.flags == flags)
        {
            mark.mask.insert(mask);
            return;
        }
        self.ignored_fdinfo.push(FanotifyIgnoredFdinfoMark {
            key: fdinfo_key,
            flags,
            mask,
        });
    }

    fn remove_ignored(&mut self, mask: FanotifyEventMask) {
        self.ignored.remove(mask);
        self.ignored_surv_modify.remove(mask);
        for mark in self.ignored_fdinfo.iter_mut() {
            mark.mask.remove(mask);
        }
        self.ignored_fdinfo.retain(|mark| !mark.mask.is_empty());
    }

    fn clear_ignored_after_modify(&mut self, listener: &FanotifyListener) {
        self.ignored.clear();
        for mark in self.ignored_fdinfo.drain(..) {
            listener.clear_fanotify_ignored_mask_after_modify(mark.key, mark.flags, mark.mask);
        }
    }

    fn is_empty(&self) -> bool {
        self.watched.is_empty() && self.ignored.is_empty() && self.ignored_surv_modify.is_empty()
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

    fn clear(&mut self) {
        self.self_mask = FanotifyEventMask::empty();
        self.child_mask = FanotifyEventMask::empty();
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

    fn applies_to(&self, context: FanotifyEventContext, child_event: bool) -> bool {
        let mark_mask = if child_event { self.child_mask } else { self.self_mask };
        if mark_mask.is_empty() {
            return false;
        }
        if context.is_dir && !mark_mask.contains(FanotifyEventMask::FAN_ONDIR) {
            return false;
        }
        true
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
