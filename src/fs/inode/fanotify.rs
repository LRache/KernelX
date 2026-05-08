use alloc::sync::{Arc, Weak};
use alloc::vec::Vec;
use bitflags::bitflags;

use crate::fs::Dentry;
use crate::fs::file::FileOps;
use crate::fs::inode::{FileType, Index};
use crate::kernel::errno::SysResult;
use crate::klib::SpinLock;

bitflags! {
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub struct FanotifyEventMask: u64 {
        const FAN_ACCESS = 0x0000_0001;
        const FAN_MODIFY = 0x0000_0002;
        const FAN_CLOSE_WRITE = 0x0000_0008;
        const FAN_CLOSE_NOWRITE = 0x0000_0010;
        const FAN_OPEN = 0x0000_0020;
        const FAN_DELETE = 0x0000_0200;
        const FAN_OPEN_EXEC = 0x0000_1000;
        const FAN_RENAME = 0x1000_0000;
        const FAN_OPEN_PERM = 0x0001_0000;
        const FAN_ACCESS_PERM = 0x0002_0000;
        const FAN_OPEN_EXEC_PERM = 0x0004_0000;
        const FAN_EVENT_ON_CHILD = 0x0800_0000;
        const FAN_ONDIR = 0x4000_0000;
    }
}

impl FanotifyEventMask {
    fn event_bits(self) -> Self {
        self & (Self::FAN_ACCESS
            | Self::FAN_MODIFY
            | Self::FAN_CLOSE_WRITE
            | Self::FAN_CLOSE_NOWRITE
            | Self::FAN_OPEN
            | Self::FAN_DELETE
            | Self::FAN_OPEN_EXEC
            | Self::FAN_OPEN_PERM
            | Self::FAN_ACCESS_PERM
            | Self::FAN_OPEN_EXEC_PERM)
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
    fn add_fanotify_mark(
        &self,
        index: Option<Index>,
        mount_id: Option<usize>,
        flags: FanotifyMarkFlags,
        mask: FanotifyEventMask,
    );
    fn remove_fanotify_mark(
        &self,
        index: Option<Index>,
        mount_id: Option<usize>,
        flags: FanotifyMarkFlags,
        mask: FanotifyEventMask,
    );
    fn queue_fanotify_event(&self, mask: FanotifyEventMask, file: Option<Arc<dyn FileOps>>);
    fn queue_fanotify_permission(&self, mask: FanotifyEventMask, file: Arc<dyn FileOps>) -> SysResult<()>;
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum FanotifyMarkScope {
    Inode(Index),
    Mount {
        fdinfo_index: Option<Index>,
        mount_id: usize,
    },
    Filesystem {
        fdinfo_index: Option<Index>,
        sno: u32,
    },
}

impl FanotifyMarkScope {
    fn index(self) -> Option<Index> {
        match self {
            Self::Inode(index) => Some(index),
            Self::Mount { fdinfo_index, .. } => fdinfo_index,
            Self::Filesystem { fdinfo_index, .. } => fdinfo_index,
        }
    }

    fn mount_id(self) -> Option<usize> {
        match self {
            Self::Inode(_) => None,
            Self::Mount { mount_id, .. } => Some(mount_id),
            Self::Filesystem { .. } => None,
        }
    }
}

struct FanotifyIgnoredMark {
    scope: FanotifyMarkScope,
    self_mask: FanotifyEventMask,
    child_mask: FanotifyEventMask,
}

#[derive(Clone, Copy)]
struct FanotifyEventContext {
    sno: Option<u32>,
    index: Option<Index>,
    parent_index: Option<Index>,
    mount_id: Option<usize>,
    is_dir: bool,
}

struct FanotifyListenerMask {
    listener: Arc<dyn FanotifyListener>,
    mark_mask: FanotifyEventMask,
    ignored_mask: FanotifyEventMask,
}

struct FanotifyListenerEvent {
    listener: Arc<dyn FanotifyListener>,
    mask: FanotifyEventMask,
}

struct FanotifyListenerMasks {
    masks: Vec<FanotifyListenerMask>,
}

struct FanotifyMark {
    listener_id: usize,
    generation: usize,
    listener: Weak<dyn FanotifyListener>,
    self_mask: FanotifyEventMask,
    child_mask: FanotifyEventMask,
    ignored: Vec<FanotifyIgnoredMark>,
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
        listener: &Arc<dyn FanotifyListener>,
        flags: FanotifyMarkFlags,
        mask: FanotifyEventMask,
        scope: FanotifyMarkScope,
    ) {
        self.state.lock().add_mark(listener, flags, mask, scope);
    }

    pub fn remove_mark(
        &self,
        listener_id: usize,
        generation: usize,
        flags: FanotifyMarkFlags,
        mask: FanotifyEventMask,
        scope: FanotifyMarkScope,
    ) {
        self.state
            .lock()
            .remove_mark(listener_id, generation, flags, mask, scope);
    }

    fn listener_masks(
        &self,
        mask: FanotifyEventMask,
        context: FanotifyEventContext,
        child_event: bool,
    ) -> Vec<FanotifyListenerMask> {
        self.state.lock().listener_masks(mask, context, child_event)
    }

    fn dentry_context(dentry: &Arc<Dentry>) -> FanotifyEventContext {
        FanotifyEventContext {
            sno: Some(dentry.sno()),
            index: Some(dentry.get_inode_index()),
            parent_index: dentry.get_parent().map(|parent| parent.get_inode_index()),
            mount_id: Some(dentry.get_mount_id()),
            is_dir: matches!(dentry.get_inode().inode_type(), Ok(FileType::Directory)),
        }
    }

    fn file_context(file: Option<&Arc<dyn FileOps>>) -> FanotifyEventContext {
        let dentry = file.and_then(|file| file.get_dentry());
        FanotifyEventContext {
            sno: dentry.map(|dentry| dentry.sno()),
            index: dentry.map(|dentry| dentry.get_inode_index()),
            parent_index: dentry.and_then(|dentry| dentry.get_parent().map(|parent| parent.get_inode_index())),
            mount_id: dentry.map(|dentry| dentry.get_mount_id()),
            is_dir: dentry
                .map(|dentry| matches!(dentry.get_inode().inode_type(), Ok(FileType::Directory)))
                .unwrap_or(false),
        }
    }
}

impl FanotifyListenerMasks {
    fn new() -> Self {
        Self { masks: Vec::new() }
    }

    fn merge(&mut self, listener_mask: FanotifyListenerMask) {
        let listener_id = listener_mask.listener.fanotify_id();
        let generation = listener_mask.listener.fanotify_generation();
        if let Some(existing) = self.masks.iter().position(|queued| {
            queued.listener.fanotify_id() == listener_id && queued.listener.fanotify_generation() == generation
        }) {
            self.masks[existing].mark_mask.insert(listener_mask.mark_mask);
            self.masks[existing].ignored_mask.insert(listener_mask.ignored_mask);
            return;
        }
        self.masks.push(listener_mask);
    }

    fn merge_fanotify(
        &mut self,
        fanotify: Option<&Arc<Fanotify>>,
        mask: FanotifyEventMask,
        context: FanotifyEventContext,
        child_event: bool,
    ) {
        if let Some(fanotify) = fanotify {
            for listener_mask in fanotify.listener_masks(mask, context, child_event) {
                self.merge(listener_mask);
            }
        }
    }

    fn into_events(self, mask: FanotifyEventMask) -> Vec<FanotifyListenerEvent> {
        self.masks
            .into_iter()
            .filter_map(|listener_mask| {
                let mut effective_mask = mask & listener_mask.mark_mask;
                effective_mask.remove(listener_mask.ignored_mask);
                if effective_mask.event_bits().is_empty() {
                    None
                } else {
                    Some(FanotifyListenerEvent {
                        listener: listener_mask.listener,
                        mask: effective_mask,
                    })
                }
            })
            .collect()
    }
}

fn fanotify_listener_events(
    mask: FanotifyEventMask,
    context: FanotifyEventContext,
    inode_fanotify: Option<&Arc<Fanotify>>,
    mount_fanotify: Option<&Arc<Fanotify>>,
    filesystem_fanotify: Option<&Arc<Fanotify>>,
    parent_fanotify: Option<&Arc<Fanotify>>,
) -> Vec<FanotifyListenerEvent> {
    if inode_fanotify.is_none()
        && mount_fanotify.is_none()
        && filesystem_fanotify.is_none()
        && parent_fanotify.is_none()
    {
        return Vec::new();
    }

    let mut listener_masks = FanotifyListenerMasks::new();
    listener_masks.merge_fanotify(inode_fanotify, mask, context, false);
    listener_masks.merge_fanotify(mount_fanotify, mask, context, false);
    listener_masks.merge_fanotify(filesystem_fanotify, mask, context, false);
    listener_masks.merge_fanotify(parent_fanotify, mask, context, true);
    listener_masks.into_events(mask)
}

#[inline(always)]
fn fanotify_listener_events_for_file(file: &Arc<dyn FileOps>, mask: FanotifyEventMask) -> Vec<FanotifyListenerEvent> {
    let inode_fanotify = file.get_inode().and_then(|inode| inode.fanotify());
    let dentry = file.get_dentry();
    let mount_fanotify = dentry.and_then(|dentry| dentry.get_mount().fanotify());
    let filesystem_fanotify = dentry.and_then(|dentry| dentry.superblock_fanotify());
    let parent_fanotify = dentry
        .and_then(|dentry| dentry.get_parent())
        .and_then(|parent| parent.get_inode().fanotify());

    fanotify_listener_events(
        mask,
        Fanotify::file_context(Some(file)),
        inode_fanotify.as_ref(),
        mount_fanotify.as_ref(),
        filesystem_fanotify.as_ref(),
        parent_fanotify.as_ref(),
    )
}

fn fanotify_listener_events_for_dentry(dentry: &Arc<Dentry>, mask: FanotifyEventMask) -> Vec<FanotifyListenerEvent> {
    let inode_fanotify = dentry.get_inode().fanotify();
    let mount_fanotify = dentry.get_mount().fanotify();
    let filesystem_fanotify = dentry.superblock_fanotify();
    let parent_fanotify = dentry.get_parent().and_then(|parent| parent.get_inode().fanotify());

    fanotify_listener_events(
        mask,
        Fanotify::dentry_context(dentry),
        inode_fanotify.as_ref(),
        mount_fanotify.as_ref(),
        filesystem_fanotify.as_ref(),
        parent_fanotify.as_ref(),
    )
}

#[inline(always)]
pub fn wait_fanotify_permission(file: &Arc<dyn FileOps>, mask: FanotifyEventMask) -> SysResult<()> {
    for event in fanotify_listener_events_for_file(file, mask) {
        event.listener.queue_fanotify_permission(event.mask, file.clone())?;
    }

    Ok(())
}

#[inline(always)]
pub fn wait_fanotify_open_exec_permission(file: &Arc<dyn FileOps>) -> SysResult<()> {
    wait_fanotify_permission(file, FanotifyEventMask::FAN_OPEN_PERM)?;
    wait_fanotify_permission(file, FanotifyEventMask::FAN_OPEN_EXEC_PERM)
}

#[inline(always)]
pub fn notify_fanotify(file: &Arc<dyn FileOps>, mask: FanotifyEventMask) {
    for event in fanotify_listener_events_for_file(file, mask) {
        event.listener.queue_fanotify_event(event.mask, Some(file.clone()));
    }
}

#[inline(always)]
pub fn notify_fanotify_dentry(dentry: &Arc<Dentry>, mask: FanotifyEventMask) {
    for event in fanotify_listener_events_for_dentry(dentry, mask) {
        event.listener.queue_fanotify_event(event.mask, None);
    }
}

impl FanotifyState {
    pub const fn new() -> Self {
        Self { marks: Vec::new() }
    }

    fn add_mark(
        &mut self,
        listener: &Arc<dyn FanotifyListener>,
        flags: FanotifyMarkFlags,
        mask: FanotifyEventMask,
        scope: FanotifyMarkScope,
    ) {
        let index = scope.index();
        let mount_id = scope.mount_id();
        let listener_id = listener.fanotify_id();
        let generation = listener.fanotify_generation();
        let ignore_mark = Self::is_ignore_mark(flags);
        let (self_mask, child_mask) = Self::split_child_mask(mask);
        if let Some(mark) = self
            .marks
            .iter_mut()
            .find(|mark| mark.listener_id == listener_id && mark.generation == generation)
        {
            if ignore_mark {
                Self::insert_ignored_mark(&mut mark.ignored, scope, mask);
            } else {
                Self::insert_event_mask(&mut mark.self_mask, &mut mark.child_mask, mask);
            }
            listener.add_fanotify_mark(index, mount_id, flags, mask);
            return;
        }

        let (mark_self_mask, mark_child_mask, ignored) = if ignore_mark {
            (
                FanotifyEventMask::empty(),
                FanotifyEventMask::empty(),
                Vec::from([FanotifyIgnoredMark {
                    scope,
                    self_mask,
                    child_mask,
                }]),
            )
        } else {
            (self_mask, child_mask, Vec::new())
        };
        self.marks.push(FanotifyMark {
            listener_id,
            generation,
            listener: Arc::downgrade(listener),
            self_mask: mark_self_mask,
            child_mask: mark_child_mask,
            ignored,
        });
        listener.add_fanotify_mark(index, mount_id, flags, mask);
    }

    fn remove_mark(
        &mut self,
        listener_id: usize,
        generation: usize,
        flags: FanotifyMarkFlags,
        mask: FanotifyEventMask,
        scope: FanotifyMarkScope,
    ) {
        let index = scope.index();
        let mount_id = scope.mount_id();
        let ignore_mark = Self::is_ignore_mark(flags);
        for mark in self
            .marks
            .iter_mut()
            .filter(|mark| mark.listener_id == listener_id && mark.generation == generation)
        {
            if let Some(listener) = mark.listener.upgrade() {
                listener.remove_fanotify_mark(index, mount_id, flags, mask);
            }
            if ignore_mark {
                Self::remove_ignored_mark(&mut mark.ignored, scope, mask);
            } else {
                Self::remove_event_mask(&mut mark.self_mask, &mut mark.child_mask, mask);
            }
        }
        self.marks
            .retain(|mark| !mark.self_mask.is_empty() || !mark.child_mask.is_empty() || !mark.ignored.is_empty());
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
            let mark_mask = Self::matching_mark_mask(mark, mask, context, child_event);
            let ignored_mask = Self::matching_ignored_mask(mark, mask, context);
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

    fn is_ignore_mark(flags: FanotifyMarkFlags) -> bool {
        flags.intersects(FanotifyMarkFlags::FAN_MARK_IGNORE | FanotifyMarkFlags::FAN_MARK_IGNORED_MASK)
    }

    fn split_child_mask(mask: FanotifyEventMask) -> (FanotifyEventMask, FanotifyEventMask) {
        let event_mask = Self::event_mask(mask);
        let child_mask = if mask.contains(FanotifyEventMask::FAN_EVENT_ON_CHILD) {
            event_mask
        } else {
            FanotifyEventMask::empty()
        };
        (event_mask, child_mask)
    }

    fn event_mask(mask: FanotifyEventMask) -> FanotifyEventMask {
        let mut event_mask = mask;
        event_mask.remove(FanotifyEventMask::FAN_EVENT_ON_CHILD);
        event_mask
    }

    fn insert_event_mask(
        self_mask: &mut FanotifyEventMask,
        child_mask: &mut FanotifyEventMask,
        mask: FanotifyEventMask,
    ) {
        let event_mask = Self::event_mask(mask);
        let child_enabled = !child_mask.is_empty() || mask.contains(FanotifyEventMask::FAN_EVENT_ON_CHILD);

        self_mask.insert(event_mask);
        if child_enabled {
            child_mask.insert(*self_mask);
        }
    }

    fn remove_event_mask(
        self_mask: &mut FanotifyEventMask,
        child_mask: &mut FanotifyEventMask,
        mask: FanotifyEventMask,
    ) {
        let event_mask = Self::event_mask(mask);

        self_mask.remove(event_mask);
        child_mask.remove(event_mask);
        if mask.contains(FanotifyEventMask::FAN_EVENT_ON_CHILD) {
            *child_mask = FanotifyEventMask::empty();
        }
    }

    fn insert_ignored_mark(ignored: &mut Vec<FanotifyIgnoredMark>, scope: FanotifyMarkScope, mask: FanotifyEventMask) {
        if let Some(ignored_mark) = ignored.iter_mut().find(|ignored_mark| ignored_mark.scope == scope) {
            Self::insert_event_mask(&mut ignored_mark.self_mask, &mut ignored_mark.child_mask, mask);
            return;
        }
        let (self_mask, child_mask) = Self::split_child_mask(mask);
        ignored.push(FanotifyIgnoredMark {
            scope,
            self_mask,
            child_mask,
        });
    }

    fn remove_ignored_mark(ignored: &mut Vec<FanotifyIgnoredMark>, scope: FanotifyMarkScope, mask: FanotifyEventMask) {
        for ignored_mark in ignored.iter_mut().filter(|ignored_mark| ignored_mark.scope == scope) {
            Self::remove_event_mask(&mut ignored_mark.self_mask, &mut ignored_mark.child_mask, mask);
        }
        ignored.retain(|ignored_mark| !ignored_mark.self_mask.is_empty() || !ignored_mark.child_mask.is_empty());
    }

    fn matching_mark_mask(
        mark: &FanotifyMark,
        mask: FanotifyEventMask,
        context: FanotifyEventContext,
        child_event: bool,
    ) -> FanotifyEventMask {
        let mark_mask = if child_event { mark.child_mask } else { mark.self_mask };
        Self::matching_event_mask(mark_mask, context, mask)
    }

    fn matching_ignored_mask(
        mark: &FanotifyMark,
        mask: FanotifyEventMask,
        context: FanotifyEventContext,
    ) -> FanotifyEventMask {
        let mut ignored_mask = FanotifyEventMask::empty();
        for ignored_mark in &mark.ignored {
            let matching_mask = match ignored_mark.scope {
                FanotifyMarkScope::Inode(ignored_index) => {
                    if Some(ignored_index) == context.index {
                        Self::matching_event_mask(ignored_mark.self_mask, context, mask)
                    } else if Some(ignored_index) == context.parent_index {
                        Self::matching_event_mask(ignored_mark.child_mask, context, mask)
                    } else {
                        FanotifyEventMask::empty()
                    }
                }
                FanotifyMarkScope::Mount { mount_id, .. } => {
                    if Some(mount_id) == context.mount_id {
                        Self::matching_event_mask(ignored_mark.self_mask, context, mask)
                    } else {
                        FanotifyEventMask::empty()
                    }
                }
                FanotifyMarkScope::Filesystem { sno, .. } => {
                    if Some(sno) == context.sno {
                        Self::matching_event_mask(ignored_mark.self_mask, context, mask)
                    } else {
                        FanotifyEventMask::empty()
                    }
                }
            };
            ignored_mask.insert(matching_mask);
        }
        ignored_mask
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
