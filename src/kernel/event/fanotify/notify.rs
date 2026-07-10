use alloc::sync::Arc;
use alloc::vec::Vec;

use crate::fs::Dentry;
use crate::fs::file::FileOps;
use crate::kernel::errno::SysResult;

use super::file::FanotifyListener;
use super::mark::{Fanotify, FanotifyEventContext, FanotifyListenerMask};
use super::types::FanotifyEventMask;

struct FanotifyListenerEvent {
    listener: Arc<FanotifyListener>,
    mask: FanotifyEventMask,
}

struct FanotifyListenerMasks {
    masks: Vec<FanotifyListenerMask>,
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
        event.listener.queue_fanotify_event(event.mask, Some(file.clone_file()));
    }
}

#[inline(always)]
pub fn notify_fanotify_dentry(dentry: &Arc<Dentry>, mask: FanotifyEventMask) {
    for event in fanotify_listener_events_for_dentry(dentry, mask) {
        event.listener.queue_fanotify_event(event.mask, None);
    }
}
