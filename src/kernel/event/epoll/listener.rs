use alloc::collections::{BTreeMap, VecDeque};
use alloc::sync::Arc;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicUsize, Ordering};

use bitflags::bitflags;
use num_enum::TryFromPrimitive;

use crate::fs::file::{FileFlags, FileOps};
use crate::fs::{Dentry, InodeOps, Mode};
use crate::kernel::errno::{Errno, SysResult};
use crate::kernel::scheduler::current;
use crate::kernel::syscall::UserStruct;
use crate::kernel::uapi::FileStat;
use crate::klib::SpinLock;

use super::super::{Event, FileEvent, WaitQueue};
use super::notifier::EpollNotifier;

static NEXT_EPOLL_LISTENER_ID: AtomicUsize = AtomicUsize::new(1);

bitflags! {
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub struct EpollEventSet: u32 {
        const EPOLLIN        = FileEvent::READ_READY.bits() as u32;
        const EPOLLPRI       = FileEvent::PRIORITY.bits() as u32;
        const EPOLLOUT       = FileEvent::WRITE_READY.bits() as u32;
        const EPOLLERR       = FileEvent::ERROR.bits() as u32;
        const EPOLLHUP       = FileEvent::HANG_UP.bits() as u32;
        const EPOLLRDHUP     = 0x00002000;
        const EPOLLEXCLUSIVE = 1 << 28;
        const EPOLLWAKEUP    = 1 << 29;
        const EPOLLONESHOT   = 1 << 30;
        const EPOLLET        = 1 << 31;
    }
}

impl EpollEventSet {
    fn to_file_event(self) -> FileEvent {
        FileEvent::from_bits_truncate((self.bits() & u16::MAX as u32) as i16)
    }

    fn from_ready_file_event(ready: FileEvent) -> Self {
        Self::from_bits_truncate(ready.bits() as u32)
    }
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct EpollEvent {
    pub events: EpollEventSet,
    pub data: u64,
}

impl UserStruct for EpollEvent {}

impl EpollEvent {
    fn ready(self, ready: FileEvent) -> Self {
        let events = EpollEventSet::from_ready_file_event(ready);
        Self {
            events,
            data: self.data,
        }
    }
}

#[repr(usize)]
#[derive(Debug, Clone, Copy, TryFromPrimitive)]
pub enum EpollCtlOp {
    Add = 1,
    Del = 2,
    Mod = 3,
}

struct EpollEntry {
    fd: usize,
    file: Arc<dyn FileOps>,
    notifiers: Vec<Arc<EpollNotifier>>,
    event: EpollEvent,
    ready_event: FileEvent,
    queued: bool,
    disabled: bool,
}

impl EpollEntry {
    fn new(fd: usize, file: Arc<dyn FileOps>, notifiers: Vec<Arc<EpollNotifier>>, event: EpollEvent) -> Self {
        Self {
            fd,
            file,
            notifiers,
            event,
            ready_event: FileEvent::empty(),
            queued: false,
            disabled: false,
        }
    }

    fn interest(&self) -> FileEvent {
        let events = self.event.events;
        events.to_file_event()
    }

    fn matches_ready(&self, ready: FileEvent) -> FileEvent {
        let interest = self.interest();
        let always_report = ready & (FileEvent::ERROR | FileEvent::HANG_UP);
        (ready & interest) | always_report
    }

    fn oneshot(&self) -> bool {
        let events = self.event.events;
        events.contains(EpollEventSet::EPOLLONESHOT)
    }

    fn edge_triggered(&self) -> bool {
        let events = self.event.events;
        events.contains(EpollEventSet::EPOLLET)
    }
}

struct EpollListenerInner {
    entries: BTreeMap<usize, EpollEntry>,
    ready: VecDeque<usize>,
    waiter: WaitQueue<Event>,
}

impl EpollListenerInner {
    fn new() -> Self {
        Self {
            entries: BTreeMap::new(),
            ready: VecDeque::new(),
            waiter: WaitQueue::new(),
        }
    }

    fn mark_ready(&mut self, fd: usize, ready: FileEvent) -> bool {
        let Some(entry) = self.entries.get_mut(&fd) else {
            return false;
        };
        if entry.disabled {
            return false;
        }

        let ready = entry.matches_ready(ready);
        if ready.is_empty() {
            return false;
        }

        entry.ready_event |= ready;
        if !entry.queued {
            entry.queued = true;
            self.ready.push_back(fd);
        }
        true
    }

    fn wake_waiters(&mut self) {
        self.waiter.wake_all(|event| match event {
            Event::Poll { waker, .. } => Event::Poll {
                event: FileEvent::READ_READY,
                waker,
            },
            _ => Event::Epoll,
        });
    }
}

pub struct EpollListener {
    id: usize,
    inner: SpinLock<EpollListenerInner>,
}

impl EpollListener {
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            id: NEXT_EPOLL_LISTENER_ID.fetch_add(1, Ordering::Relaxed),
            inner: SpinLock::new(EpollListenerInner::new(), "EpollListener::inner"),
        })
    }

    pub(super) fn id(&self) -> usize {
        self.id
    }

    pub fn add(
        self: &Arc<Self>,
        fd: usize,
        file: Arc<dyn FileOps>,
        notifiers: Vec<Arc<EpollNotifier>>,
        event: EpollEvent,
    ) -> SysResult<()> {
        let mut inner = self.inner.lock();
        if inner.entries.contains_key(&fd) {
            return Err(Errno::EEXIST);
        }

        notifiers.iter().for_each(|notifier| notifier.add_listener(self));
        inner.entries.insert(fd, EpollEntry::new(fd, file, notifiers, event));
        drop(inner);

        self.refresh_fd(fd);
        Ok(())
    }

    pub fn modify(&self, fd: usize, event: EpollEvent) -> SysResult<()> {
        {
            let mut inner = self.inner.lock();
            let entry = inner.entries.get_mut(&fd).ok_or(Errno::ENOENT)?;
            entry.event = event;
            entry.ready_event = FileEvent::empty();
            entry.queued = false;
            entry.disabled = false;
            inner.ready.retain(|ready_fd| *ready_fd != fd);
        }

        self.refresh_fd(fd);
        Ok(())
    }

    pub fn delete(&self, fd: usize) -> SysResult<()> {
        let mut inner = self.inner.lock();
        let entry = inner.entries.remove(&fd).ok_or(Errno::ENOENT)?;
        inner.ready.retain(|ready_fd| *ready_fd != fd);
        let stale_notifiers = entry
            .notifiers
            .iter()
            .filter(|notifier| {
                !inner.entries.values().any(|other| {
                    other
                        .notifiers
                        .iter()
                        .any(|other_notifier| other_notifier.id() == notifier.id())
                })
            })
            .cloned()
            .collect::<Vec<_>>();
        drop(inner);

        stale_notifiers
            .into_iter()
            .for_each(|notifier| notifier.remove_listener(self.id()));
        Ok(())
    }

    pub fn clear(&self) {
        let entries = {
            let mut inner = self.inner.lock();
            inner.ready.clear();
            inner.entries.split_off(&0)
        };

        entries.into_values().for_each(|entry| {
            entry
                .notifiers
                .iter()
                .for_each(|notifier| notifier.remove_listener(self.id()));
        });
    }

    pub(super) fn notify(&self, notifier_id: usize, event: FileEvent) {
        let mut inner = self.inner.lock();
        let fds = inner
            .entries
            .iter()
            .filter_map(|(fd, entry)| {
                entry
                    .notifiers
                    .iter()
                    .any(|notifier| notifier.id() == notifier_id)
                    .then_some(*fd)
            })
            .collect::<Vec<_>>();

        let mut wake = false;
        fds.into_iter().for_each(|fd| {
            wake |= inner.mark_ready(fd, event);
        });
        if wake {
            inner.wake_waiters();
        }
    }

    fn refresh_fd(&self, fd: usize) {
        let file_and_interest = {
            let inner = self.inner.lock();
            inner
                .entries
                .get(&fd)
                .map(|entry| (entry.file.clone(), entry.interest()))
        };
        let Some((file, interest)) = file_and_interest else {
            return;
        };
        let Ok(Some(ready)) = file.poll_event(interest) else {
            return;
        };

        let mut inner = self.inner.lock();
        if inner.mark_ready(fd, ready) {
            inner.wake_waiters();
        }
    }

    fn refresh_all(&self) {
        let files = {
            let inner = self.inner.lock();
            inner
                .entries
                .values()
                .filter(|entry| !entry.disabled && !entry.edge_triggered())
                .map(|entry| (entry.fd, entry.file.clone(), entry.interest()))
                .collect::<Vec<_>>()
        };

        let mut ready = Vec::new();
        files.into_iter().for_each(|(fd, file, interest)| {
            if let Ok(Some(event)) = file.poll_event(interest) {
                ready.push((fd, event));
            }
        });

        if ready.is_empty() {
            return;
        }

        let mut inner = self.inner.lock();
        let mut wake = false;
        ready.into_iter().for_each(|(fd, event)| {
            wake |= inner.mark_ready(fd, event);
        });
        if wake {
            inner.wake_waiters();
        }
    }

    pub fn collect_ready(&self, max_events: usize) -> Vec<EpollEvent> {
        self.refresh_all();

        let mut events = Vec::new();
        let mut inner = self.inner.lock();
        while events.len() < max_events {
            let Some(fd) = inner.ready.pop_front() else {
                break;
            };
            let Some(entry) = inner.entries.get_mut(&fd) else {
                continue;
            };

            entry.queued = false;
            if entry.disabled || entry.ready_event.is_empty() {
                continue;
            }

            let ready = entry.ready_event;
            entry.ready_event = FileEvent::empty();
            events.push(entry.event.ready(ready));

            if entry.oneshot() {
                entry.disabled = true;
            }
        }

        events
    }

    pub fn has_ready(&self) -> bool {
        self.refresh_all();
        !self.inner.lock().ready.is_empty()
    }

    pub fn wait_current(&self) {
        self.inner.lock().waiter.wait_current(Event::Epoll);
    }

    pub fn wait_poll(&self, waker: usize) {
        self.inner.lock().waiter.wait(
            current::task().clone(),
            Event::Poll {
                event: FileEvent::READ_READY,
                waker,
            },
        );
    }

    pub fn wait_cancel(&self) {
        self.inner.lock().waiter.remove(current::task());
    }
}

pub struct EpollFile {
    listener: Arc<EpollListener>,
    notifier: Arc<EpollNotifier>,
    flags: SpinLock<FileFlags>,
}

impl EpollFile {
    pub fn new() -> Self {
        Self {
            listener: EpollListener::new(),
            notifier: Arc::new(EpollNotifier::new()),
            flags: SpinLock::new(
                FileFlags {
                    readable: true,
                    writable: false,
                    blocked: true,
                    append: false,
                    direct: false,
                },
                "EpollFile::flags",
            ),
        }
    }

    pub fn listener(&self) -> &Arc<EpollListener> {
        &self.listener
    }

    pub fn set_blocked(&self, blocked: bool) {
        self.flags.lock().blocked = blocked;
    }
}

impl Drop for EpollFile {
    fn drop(&mut self) {
        self.listener.clear();
    }
}

impl FileOps for EpollFile {
    fn read(&self, _buf: &mut [u8]) -> SysResult<usize> {
        Err(Errno::EINVAL)
    }

    fn write(&self, _buf: &[u8]) -> SysResult<usize> {
        Err(Errno::EINVAL)
    }

    fn flags(&self) -> FileFlags {
        *self.flags.lock()
    }

    fn fstat(&self) -> SysResult<FileStat> {
        let mut kstat = FileStat::empty();
        kstat.st_ino = self as *const Self as *const () as u64;
        kstat.st_mode = (Mode::S_IFREG | Mode::S_IRUSR).bits();
        kstat.st_nlink = 1;
        Ok(kstat)
    }

    fn fsync(&self) -> SysResult<()> {
        Ok(())
    }

    fn get_inode(&self) -> Option<&Arc<dyn InodeOps>> {
        None
    }

    fn get_dentry(&self) -> Option<&Arc<Dentry>> {
        None
    }

    fn poll_event(&self, event: FileEvent) -> SysResult<Option<FileEvent>> {
        if event.contains(FileEvent::READ_READY) && self.listener.has_ready() {
            Ok(Some(FileEvent::READ_READY))
        } else {
            Ok(None)
        }
    }

    fn wait_event(&self, waker: usize, event: FileEvent) -> SysResult<Option<FileEvent>> {
        if let Some(ready) = self.poll_event(event)? {
            return Ok(Some(ready));
        }
        if event.contains(FileEvent::READ_READY) {
            self.listener.wait_poll(waker);
        }
        Ok(None)
    }

    fn wait_event_cancel(&self) {
        self.listener.wait_cancel();
    }

    fn epoll_notifier(&self) -> Option<Arc<EpollNotifier>> {
        Some(self.notifier.clone())
    }

    fn set_flags(&self, flags: FileFlags) {
        self.set_blocked(flags.blocked);
    }

    fn type_name(&self) -> &'static str {
        "epoll"
    }
}
