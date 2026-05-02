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
        const FAN_OPEN_EXEC = 0x0000_1000;
        const FAN_OPEN_PERM = 0x0001_0000;
        const FAN_ACCESS_PERM = 0x0002_0000;
        const FAN_OPEN_EXEC_PERM = 0x0004_0000;
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

    fn matching_listeners(
        &self,
        mask: FanotifyEventMask,
        context: FanotifyEventContext,
        child_event: bool,
    ) -> Vec<Arc<dyn FanotifyListener>> {
        let mut state = self.state.lock();
        if child_event {
            state.listeners_for_child_mask(mask, context)
        } else {
            state.listeners_for_mask(mask, context)
        }
    }

    fn ignores_listener(
        &self,
        listener_id: usize,
        generation: usize,
        mask: FanotifyEventMask,
        context: FanotifyEventContext,
    ) -> bool {
        self.state
            .lock()
            .ignores_listener(listener_id, generation, mask, context)
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

fn push_unique_listener(listeners: &mut Vec<Arc<dyn FanotifyListener>>, listener: Arc<dyn FanotifyListener>) {
    let listener_id = listener.fanotify_id();
    let generation = listener.fanotify_generation();
    if listeners
        .iter()
        .any(|queued| queued.fanotify_id() == listener_id && queued.fanotify_generation() == generation)
    {
        return;
    }
    listeners.push(listener);
}

fn fanotify_ignores_listener(
    fanotify: Option<&Arc<Fanotify>>,
    listener_id: usize,
    generation: usize,
    mask: FanotifyEventMask,
    context: FanotifyEventContext,
) -> bool {
    if let Some(fanotify) = fanotify {
        fanotify.ignores_listener(listener_id, generation, mask, context)
    } else {
        false
    }
}

#[inline(always)]
fn fanotify_listeners_for_file(file: &Arc<dyn FileOps>, mask: FanotifyEventMask) -> Vec<Arc<dyn FanotifyListener>> {
    let inode_fanotify = file.get_inode().and_then(|inode| inode.fanotify());
    let dentry = file.get_dentry();
    let mount_fanotify = dentry.and_then(|dentry| dentry.get_mount().fanotify());
    let filesystem_fanotify = dentry.and_then(|dentry| dentry.superblock_fanotify());
    let parent_fanotify = dentry
        .and_then(|dentry| dentry.get_parent())
        .and_then(|parent| parent.get_inode().fanotify());

    if inode_fanotify.is_none()
        && mount_fanotify.is_none()
        && filesystem_fanotify.is_none()
        && parent_fanotify.is_none()
    {
        return Vec::new();
    }

    let context = Fanotify::file_context(Some(file));
    let mut listeners = Vec::new();
    if let Some(fanotify) = &inode_fanotify {
        for listener in fanotify.matching_listeners(mask, context, false) {
            push_unique_listener(&mut listeners, listener);
        }
    }
    if let Some(fanotify) = &mount_fanotify {
        for listener in fanotify.matching_listeners(mask, context, false) {
            push_unique_listener(&mut listeners, listener);
        }
    }
    if let Some(fanotify) = &filesystem_fanotify {
        for listener in fanotify.matching_listeners(mask, context, false) {
            push_unique_listener(&mut listeners, listener);
        }
    }
    if let Some(fanotify) = &parent_fanotify {
        for listener in fanotify.matching_listeners(mask, context, true) {
            push_unique_listener(&mut listeners, listener);
        }
    }

    listeners
        .into_iter()
        .filter(|listener| {
            let listener_id = listener.fanotify_id();
            let generation = listener.fanotify_generation();
            !fanotify_ignores_listener(inode_fanotify.as_ref(), listener_id, generation, mask, context)
                && !fanotify_ignores_listener(mount_fanotify.as_ref(), listener_id, generation, mask, context)
                && !fanotify_ignores_listener(filesystem_fanotify.as_ref(), listener_id, generation, mask, context)
                && !fanotify_ignores_listener(parent_fanotify.as_ref(), listener_id, generation, mask, context)
        })
        .collect()
}

fn fanotify_listeners_for_dentry(dentry: &Arc<Dentry>, mask: FanotifyEventMask) -> Vec<Arc<dyn FanotifyListener>> {
    let inode_fanotify = dentry.get_inode().fanotify();
    let mount_fanotify = dentry.get_mount().fanotify();
    let filesystem_fanotify = dentry.superblock_fanotify();
    let parent_fanotify = dentry.get_parent().and_then(|parent| parent.get_inode().fanotify());

    if inode_fanotify.is_none()
        && mount_fanotify.is_none()
        && filesystem_fanotify.is_none()
        && parent_fanotify.is_none()
    {
        return Vec::new();
    }

    let context = Fanotify::dentry_context(dentry);
    let mut listeners = Vec::new();
    if let Some(fanotify) = &inode_fanotify {
        for listener in fanotify.matching_listeners(mask, context, false) {
            push_unique_listener(&mut listeners, listener);
        }
    }
    if let Some(fanotify) = &mount_fanotify {
        for listener in fanotify.matching_listeners(mask, context, false) {
            push_unique_listener(&mut listeners, listener);
        }
    }
    if let Some(fanotify) = &filesystem_fanotify {
        for listener in fanotify.matching_listeners(mask, context, false) {
            push_unique_listener(&mut listeners, listener);
        }
    }
    if let Some(fanotify) = &parent_fanotify {
        for listener in fanotify.matching_listeners(mask, context, true) {
            push_unique_listener(&mut listeners, listener);
        }
    }

    listeners
        .into_iter()
        .filter(|listener| {
            let listener_id = listener.fanotify_id();
            let generation = listener.fanotify_generation();
            !fanotify_ignores_listener(inode_fanotify.as_ref(), listener_id, generation, mask, context)
                && !fanotify_ignores_listener(mount_fanotify.as_ref(), listener_id, generation, mask, context)
                && !fanotify_ignores_listener(filesystem_fanotify.as_ref(), listener_id, generation, mask, context)
                && !fanotify_ignores_listener(parent_fanotify.as_ref(), listener_id, generation, mask, context)
        })
        .collect()
}

#[inline(always)]
pub fn wait_fanotify_permission(file: &Arc<dyn FileOps>, mask: FanotifyEventMask) -> SysResult<()> {
    for listener in fanotify_listeners_for_file(file, mask) {
        listener.queue_fanotify_permission(mask, file.clone())?;
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
    for listener in fanotify_listeners_for_file(file, mask) {
        listener.queue_fanotify_event(mask, Some(file.clone()));
    }
}

#[inline(always)]
pub fn notify_fanotify_dentry(dentry: &Arc<Dentry>, mask: FanotifyEventMask) {
    for listener in fanotify_listeners_for_dentry(dentry, mask) {
        listener.queue_fanotify_event(mask, None);
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

    fn listeners_for_mask(
        &mut self,
        mask: FanotifyEventMask,
        context: FanotifyEventContext,
    ) -> Vec<Arc<dyn FanotifyListener>> {
        let mut listeners = Vec::new();
        self.marks.retain(|mark| {
            let Some(listener) = mark.listener.upgrade() else {
                return false;
            };
            if listener.fanotify_generation() != mark.generation {
                return false;
            }
            if Self::mark_matches(mark, mask, context, false) {
                listeners.push(listener);
            }
            true
        });
        listeners
    }

    fn listeners_for_child_mask(
        &mut self,
        mask: FanotifyEventMask,
        context: FanotifyEventContext,
    ) -> Vec<Arc<dyn FanotifyListener>> {
        let mut listeners = Vec::new();
        self.marks.retain(|mark| {
            let Some(listener) = mark.listener.upgrade() else {
                return false;
            };
            if listener.fanotify_generation() != mark.generation {
                return false;
            }
            if Self::mark_matches(mark, mask, context, true) {
                listeners.push(listener);
            }
            true
        });
        listeners
    }

    fn ignores_listener(
        &mut self,
        listener_id: usize,
        generation: usize,
        mask: FanotifyEventMask,
        context: FanotifyEventContext,
    ) -> bool {
        let mut ignored = false;
        self.marks.retain(|mark| {
            let Some(listener) = mark.listener.upgrade() else {
                return false;
            };
            if listener.fanotify_generation() != mark.generation {
                return false;
            }
            if mark.listener_id == listener_id && mark.generation == generation && Self::is_ignored(mark, context, mask)
            {
                ignored = true;
            }
            true
        });
        ignored
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

    fn mark_matches(
        mark: &FanotifyMark,
        mask: FanotifyEventMask,
        context: FanotifyEventContext,
        child_event: bool,
    ) -> bool {
        if Self::is_ignored(mark, context, mask) {
            return false;
        }
        let mark_mask = if child_event { mark.child_mask } else { mark.self_mask };
        Self::mask_matches_event(mark_mask, context, mask)
    }

    fn is_ignored(mark: &FanotifyMark, context: FanotifyEventContext, mask: FanotifyEventMask) -> bool {
        mark.ignored.iter().any(|ignored_mark| match ignored_mark.scope {
            FanotifyMarkScope::Inode(ignored_index) => {
                (Some(ignored_index) == context.index
                    && Self::mask_matches_event(ignored_mark.self_mask, context, mask))
                    || (Some(ignored_index) == context.parent_index
                        && Self::mask_matches_event(ignored_mark.child_mask, context, mask))
            }
            FanotifyMarkScope::Mount { mount_id, .. } => {
                Some(mount_id) == context.mount_id && Self::mask_matches_event(ignored_mark.self_mask, context, mask)
            }
            FanotifyMarkScope::Filesystem { sno, .. } => {
                Some(sno) == context.sno && Self::mask_matches_event(ignored_mark.self_mask, context, mask)
            }
        })
    }

    fn mask_matches_event(
        mark_mask: FanotifyEventMask,
        context: FanotifyEventContext,
        event_mask: FanotifyEventMask,
    ) -> bool {
        if context.is_dir && !mark_mask.contains(FanotifyEventMask::FAN_ONDIR) {
            return false;
        }
        mark_mask.intersects(event_mask)
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
