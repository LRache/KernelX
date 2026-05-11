use alloc::string::String;
use alloc::sync::Arc;
use alloc::vec::Vec;
use core::fmt::Write;
use core::sync::atomic::{AtomicUsize, Ordering};

use crate::fs::file::FileOps;
use crate::fs::inode::Index;
use crate::kernel::errno::{Errno, SysResult};
use crate::kernel::event::{EpollNotifier, Event, FileEvent, WaitQueue};
use crate::kernel::scheduler::current;
use crate::klib::SpinLock;

use super::super::types::{FanotifyEventMask, FanotifyFdinfoKey, FanotifyMarkFlags};
use super::event::FanotifyEvent;
use super::permission::FanotifyPermission;

static NEXT_FANOTIFY_ID: AtomicUsize = AtomicUsize::new(1);

#[derive(Clone, Copy)]
struct FanotifyFdinfoMark {
    key: FanotifyFdinfoKey,
    flags: FanotifyMarkFlags,
    mask: FanotifyEventMask,
    ignored_mask: FanotifyEventMask,
}

pub struct FanotifyListener {
    pub(super) id: usize,
    generation: AtomicUsize,
    pub(super) fd_count: AtomicUsize,
    pub(super) report_dfid_name: bool,
    pub(super) unprivileged: bool,
    marks: SpinLock<Vec<FanotifyFdinfoMark>>,
    pub(super) pending: SpinLock<Vec<FanotifyEvent>>,
    pub(super) responses: SpinLock<Vec<(i32, Arc<FanotifyPermission>)>>,
    pub(super) waiter: SpinLock<WaitQueue<Event>>,
    pub(super) epoll_notifier: Arc<EpollNotifier>,
}

impl FanotifyListener {
    const RESPONSE_ALLOW: u32 = 0x01;
    const RESPONSE_DENY: u32 = 0x02;

    pub(super) fn new(report_dfid_name: bool, unprivileged: bool) -> Self {
        Self {
            id: NEXT_FANOTIFY_ID.fetch_add(1, Ordering::Relaxed),
            generation: AtomicUsize::new(0),
            fd_count: AtomicUsize::new(0),
            report_dfid_name,
            unprivileged,
            marks: SpinLock::new(Vec::new(), "FanotifyListener::marks"),
            pending: SpinLock::new(Vec::new(), "FanotifyListener::pending"),
            responses: SpinLock::new(Vec::new(), "FanotifyListener::responses"),
            waiter: SpinLock::new(WaitQueue::new(), "FanotifyListener::waiter"),
            epoll_notifier: Arc::new(EpollNotifier::new()),
        }
    }

    pub(in crate::kernel::event::fanotify) fn fanotify_generation(&self) -> usize {
        self.generation.load(Ordering::Acquire)
    }

    pub(in crate::kernel::event::fanotify) fn fanotify_id(&self) -> usize {
        self.id
    }

    pub(super) fn flush_marks(&self) {
        self.generation.fetch_add(1, Ordering::AcqRel);
        self.marks.lock().clear();
        for event in self.pending.lock().drain(..) {
            if let Some(permission) = event.permission {
                permission.respond(Err(Errno::EIO));
            }
        }
        for (_, permission) in self.responses.lock().drain(..) {
            permission.respond(Err(Errno::EIO));
        }
    }

    pub(super) fn pop_event(&self, blocked: bool) -> SysResult<FanotifyEvent> {
        loop {
            let mut pending = self.pending.lock();
            if !pending.is_empty() {
                return Ok(pending.remove(0));
            }

            if !blocked {
                return Err(Errno::EAGAIN);
            }

            self.waiter.lock().wait_current(Event::ReadReady);
            drop(pending);

            current::schedule();
            match current::task().take_wakeup_event().unwrap() {
                Event::ReadReady => {}
                Event::Signal => {
                    self.waiter.lock().remove(current::task());
                    return Err(Errno::EINTR);
                }
                event => unreachable!("unexpected event while waiting on fanotify read: {:?}", event),
            }
        }
    }

    fn queue_permission(&self, mask: FanotifyEventMask, file: Arc<dyn FileOps>) -> SysResult<()> {
        if self.unprivileged {
            return Err(Errno::EPERM);
        }
        let permission = Arc::new(FanotifyPermission::new());
        self.pending.lock().push(FanotifyEvent {
            mask,
            file: Some(file),
            pid: current::pid() as i32,
            permission: Some(permission.clone()),
        });
        self.waiter.lock().wake_all(|event| event);
        self.epoll_notifier.notify(FileEvent::READ_READY);
        permission.wait()
    }

    fn queue_event(&self, event: FanotifyEvent) {
        let mut pending = self.pending.lock();
        if let Some(pending_event) = pending
            .iter_mut()
            .find(|pending_event| pending_event.merges_with(&event))
        {
            pending_event.merge(&event);
            return;
        }

        pending.push(event);
        drop(pending);
        self.waiter.lock().wake_all(|event| event);
        self.epoll_notifier.notify(FileEvent::READ_READY);
    }

    pub(super) fn respond(&self, fd: i32, response: u32) -> SysResult<()> {
        let result = match response & (Self::RESPONSE_ALLOW | Self::RESPONSE_DENY) {
            Self::RESPONSE_ALLOW => Ok(()),
            Self::RESPONSE_DENY => Err(Errno::EPERM),
            _ => return Err(Errno::EINVAL),
        };

        let mut responses = self.responses.lock();
        let pos = responses
            .iter()
            .position(|(response_fd, _)| *response_fd == fd)
            .ok_or(Errno::ENOENT)?;
        let (_, permission) = responses.remove(pos);
        drop(responses);

        permission.respond(result);
        Ok(())
    }

    fn fdinfo_mark_flags(flags: FanotifyMarkFlags) -> FanotifyMarkFlags {
        let mut flags = flags;
        flags.remove(
            FanotifyMarkFlags::FAN_MARK_ADD | FanotifyMarkFlags::FAN_MARK_REMOVE | FanotifyMarkFlags::FAN_MARK_FLUSH,
        );
        flags
    }

    fn fdinfo_mark_scope_flags(flags: FanotifyMarkFlags) -> FanotifyMarkFlags {
        let mut flags = Self::fdinfo_mark_flags(flags);
        flags.remove(
            FanotifyMarkFlags::FAN_MARK_IGNORE
                | FanotifyMarkFlags::FAN_MARK_IGNORED_MASK
                | FanotifyMarkFlags::FAN_MARK_IGNORED_SURV_MODIFY,
        );
        flags
    }

    fn add_mark(&self, key: FanotifyFdinfoKey, flags: FanotifyMarkFlags, mask: FanotifyEventMask) {
        let flags = Self::fdinfo_mark_flags(flags);
        let scope_flags = Self::fdinfo_mark_scope_flags(flags);
        let is_ignore_mark = flags.is_ignore_mark();
        let mut marks = self.marks.lock();
        if let Some(mark) = marks
            .iter_mut()
            .find(|mark| mark.key == key && Self::fdinfo_mark_scope_flags(mark.flags) == scope_flags)
        {
            if is_ignore_mark {
                mark.ignored_mask.insert(mask);
            } else {
                mark.mask.insert(mask);
            }
            return;
        }

        marks.push(FanotifyFdinfoMark {
            key,
            flags: if is_ignore_mark { scope_flags } else { flags },
            mask: if is_ignore_mark {
                FanotifyEventMask::empty()
            } else {
                mask
            },
            ignored_mask: if is_ignore_mark {
                mask
            } else {
                FanotifyEventMask::empty()
            },
        });
    }

    fn remove_mark(&self, key: FanotifyFdinfoKey, flags: FanotifyMarkFlags, mask: FanotifyEventMask) {
        let flags = Self::fdinfo_mark_flags(flags);
        let scope_flags = Self::fdinfo_mark_scope_flags(flags);
        let is_ignore_mark = flags.is_ignore_mark();
        let mut marks = self.marks.lock();
        for mark in marks
            .iter_mut()
            .filter(|mark| mark.key == key && Self::fdinfo_mark_scope_flags(mark.flags) == scope_flags)
        {
            if is_ignore_mark {
                mark.ignored_mask.remove(mask);
            } else {
                mark.mask.remove(mask);
            }
        }
        marks.retain(|mark| !mark.mask.is_empty() || !mark.ignored_mask.is_empty());
    }

    pub(super) fn fdinfo(&self) -> String {
        let mut content = String::new();
        for mark in self.marks.lock().iter() {
            let index = mark.key.index().unwrap_or(Index { sno: 0, ino: 0 });
            let _ = writeln!(
                content,
                "fanotify ino:{:x} sdev:{:x} mflags:{:x} mask:{:x} ignored_mask:{:x}",
                index.ino,
                index.sno,
                mark.flags.bits(),
                mark.mask.bits(),
                mark.ignored_mask.bits(),
            );
        }
        content
    }
    pub(in crate::kernel::event::fanotify) fn add_fanotify_mark(
        &self,
        key: FanotifyFdinfoKey,
        flags: FanotifyMarkFlags,
        mask: FanotifyEventMask,
    ) {
        self.add_mark(key, flags, mask);
    }

    pub(in crate::kernel::event::fanotify) fn remove_fanotify_mark(
        &self,
        key: FanotifyFdinfoKey,
        flags: FanotifyMarkFlags,
        mask: FanotifyEventMask,
    ) {
        self.remove_mark(key, flags, mask);
    }

    pub(in crate::kernel::event::fanotify) fn queue_fanotify_event(
        &self,
        mask: FanotifyEventMask,
        file: Option<Arc<dyn FileOps>>,
    ) {
        self.queue_event(FanotifyEvent {
            mask,
            file,
            pid: current::pid() as i32,
            permission: None,
        });
    }

    pub(in crate::kernel::event::fanotify) fn queue_fanotify_permission(
        &self,
        mask: FanotifyEventMask,
        file: Arc<dyn FileOps>,
    ) -> SysResult<()> {
        self.queue_permission(mask, file)
    }
}
