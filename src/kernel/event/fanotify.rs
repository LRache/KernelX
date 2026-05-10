use alloc::string::String;
use alloc::sync::Arc;
use alloc::vec::Vec;
use core::fmt::Write;
use core::sync::atomic::{AtomicUsize, Ordering};

use crate::fs::file::{FileFlags, FileOps};
use crate::fs::inode::{FanotifyEventMask, FanotifyListener, FanotifyMarkFlags, Index};
use crate::fs::{Dentry, InodeOps, Mode};
use crate::kernel::errno::{Errno, SysResult};
use crate::kernel::event::{EpollNotifier, Event, FileEvent, WaitQueue};
use crate::kernel::mm::ubuf::UAddrSpaceBuffer;
use crate::kernel::scheduler::current;
use crate::kernel::scheduler::current::{copy_from_user, copy_to_user};
use crate::kernel::task::fdtable::FDFlags;
use crate::kernel::uapi::FileStat;
use crate::klib::SpinLock;

static NEXT_FANOTIFY_ID: AtomicUsize = AtomicUsize::new(1);

struct FanotifyPermission {
    response: SpinLock<Option<SysResult<()>>>,
    waiter: SpinLock<WaitQueue<Event>>,
}

impl FanotifyPermission {
    fn new() -> Self {
        Self {
            response: SpinLock::new(None, "FanotifyPermission::response"),
            waiter: SpinLock::new(WaitQueue::new(), "FanotifyPermission::waiter"),
        }
    }

    fn wait(&self) -> SysResult<()> {
        loop {
            let response = self.response.lock();
            if let Some(response) = *response {
                return response;
            }

            self.waiter.lock().wait_current(Event::FanotifyPermission);
            drop(response);

            current::schedule();
            match current::task().take_wakeup_event().unwrap() {
                Event::FanotifyPermission => {}
                Event::Signal => {
                    self.waiter.lock().remove(current::task());
                    return Err(Errno::EINTR);
                }
                event => unreachable!("unexpected event while waiting on fanotify permission: {:?}", event),
            }
        }
    }

    fn respond(&self, response: SysResult<()>) {
        *self.response.lock() = Some(response);
        self.waiter.lock().wake_all(|event| event);
    }
}

#[repr(C)]
#[derive(Clone, Copy)]
struct FanotifyEventMetadata {
    event_len: u32,
    vers: u8,
    reserved: u8,
    metadata_len: u16,
    mask: u64,
    fd: i32,
    pid: i32,
}

impl FanotifyEventMetadata {
    const VERSION: u8 = 3;
    const SIZE: usize = core::mem::size_of::<Self>();

    fn new(mask: FanotifyEventMask, fd: i32, pid: i32, event_len: usize) -> Self {
        Self {
            event_len: event_len as u32,
            vers: Self::VERSION,
            reserved: 0,
            metadata_len: Self::SIZE as u16,
            mask: mask.bits(),
            fd,
            pid,
        }
    }

    fn write_to(&self, buf: &mut [u8]) {
        buf[0..4].copy_from_slice(&self.event_len.to_ne_bytes());
        buf[4] = self.vers;
        buf[5] = self.reserved;
        buf[6..8].copy_from_slice(&self.metadata_len.to_ne_bytes());
        buf[8..16].copy_from_slice(&self.mask.to_ne_bytes());
        buf[16..20].copy_from_slice(&self.fd.to_ne_bytes());
        buf[20..24].copy_from_slice(&self.pid.to_ne_bytes());
    }
}

struct FanotifyDfidNameInfo<'a> {
    parent_index: Index,
    name: &'a str,
}

impl<'a> FanotifyDfidNameInfo<'a> {
    const EVENT_INFO_TYPE: u8 = 2;
    const EVENT_INFO_HEADER_SIZE: usize = 4;
    const FSID_SIZE: usize = 8;
    const FILE_HANDLE_HEADER_SIZE: usize = 8;
    const FILE_HANDLE_BYTES: usize = core::mem::size_of::<u32>() * 2;
    const FILE_HANDLE_TYPE_INODE: i32 = 1;

    fn from_file(file: Option<&'a Arc<dyn FileOps>>) -> Self {
        let dentry = file.and_then(|file| file.get_dentry());
        let parent = dentry.and_then(|dentry| dentry.get_parent());
        let parent_index = parent
            .as_ref()
            .map(|parent| parent.get_inode_index())
            .or_else(|| dentry.map(|dentry| dentry.get_inode_index()))
            .unwrap_or(Index { sno: 0, ino: 0 });
        let name = dentry.map(|dentry| dentry.name()).unwrap_or("");

        Self { parent_index, name }
    }

    fn len(&self) -> usize {
        Self::EVENT_INFO_HEADER_SIZE
            + Self::FSID_SIZE
            + Self::FILE_HANDLE_HEADER_SIZE
            + Self::FILE_HANDLE_BYTES
            + self.name.len()
            + 1
    }

    fn write_to(&self, buf: &mut [u8]) {
        let len = self.len();
        let mut offset = 0;

        buf[offset] = Self::EVENT_INFO_TYPE;
        buf[offset + 1] = 0;
        buf[offset + 2..offset + 4].copy_from_slice(&(len as u16).to_ne_bytes());
        offset += Self::EVENT_INFO_HEADER_SIZE;

        buf[offset..offset + 4].copy_from_slice(&self.parent_index.sno.to_ne_bytes());
        buf[offset + 4..offset + 8].copy_from_slice(&0u32.to_ne_bytes());
        offset += Self::FSID_SIZE;

        buf[offset..offset + 4].copy_from_slice(&(Self::FILE_HANDLE_BYTES as u32).to_ne_bytes());
        buf[offset + 4..offset + 8].copy_from_slice(&Self::FILE_HANDLE_TYPE_INODE.to_ne_bytes());
        offset += Self::FILE_HANDLE_HEADER_SIZE;

        buf[offset..offset + 4].copy_from_slice(&self.parent_index.sno.to_ne_bytes());
        buf[offset + 4..offset + 8].copy_from_slice(&self.parent_index.ino.to_ne_bytes());
        offset += Self::FILE_HANDLE_BYTES;

        buf[offset..offset + self.name.len()].copy_from_slice(self.name.as_bytes());
    }
}

#[derive(Clone)]
struct FanotifyEvent {
    mask: FanotifyEventMask,
    file: Option<Arc<dyn FileOps>>,
    pid: i32,
    permission: Option<Arc<FanotifyPermission>>,
}

impl FanotifyEvent {
    const NOFD: i32 = -1;

    fn align_len(len: usize) -> usize {
        (len + 7) & !7
    }

    fn encoded_len(&self, inner: &FanotifyInner) -> usize {
        let info_len = if inner.report_dfid_name {
            FanotifyDfidNameInfo::from_file(self.file.as_ref()).len()
        } else {
            0
        };
        Self::align_len(FanotifyEventMetadata::SIZE + info_len)
    }

    fn target_matches(&self, other: &Self) -> bool {
        match (self.file.as_ref(), other.file.as_ref()) {
            (Some(self_file), Some(other_file)) => match (self_file.get_dentry(), other_file.get_dentry()) {
                (Some(self_dentry), Some(other_dentry)) => {
                    self_dentry.get_inode_index() == other_dentry.get_inode_index()
                }
                _ => Arc::ptr_eq(self_file, other_file),
            },
            _ => false,
        }
    }

    fn merges_with(&self, other: &Self) -> bool {
        self.permission.is_none() && other.permission.is_none() && self.pid == other.pid && self.target_matches(other)
    }

    fn merge(&mut self, other: &Self) {
        self.mask.insert(other.mask);
    }

    fn write_to(mut self, inner: &FanotifyInner, buf: &mut [u8]) -> SysResult<usize> {
        let event_len = self.encoded_len(inner);
        if buf.len() < event_len {
            return Err(Errno::EINVAL);
        }

        for byte in &mut buf[..event_len] {
            *byte = 0;
        }

        let pid = if inner.unprivileged && current::pid() as i32 != self.pid {
            0
        } else {
            self.pid
        };
        let fd = if inner.unprivileged || (inner.report_dfid_name && self.permission.is_none()) {
            Self::NOFD
        } else if let Some(file) = self.file.take() {
            let fd = current::fdtable().lock().push(file, FDFlags::empty())? as i32;
            if let Some(permission) = self.permission {
                inner.responses.lock().push((fd, permission));
            }
            fd
        } else {
            Self::NOFD
        };

        FanotifyEventMetadata::new(self.mask, fd, pid, event_len).write_to(&mut buf[..FanotifyEventMetadata::SIZE]);
        if inner.report_dfid_name {
            FanotifyDfidNameInfo::from_file(self.file.as_ref())
                .write_to(&mut buf[FanotifyEventMetadata::SIZE..event_len]);
        }

        Ok(event_len)
    }
}

#[derive(Clone, Copy)]
struct FanotifyFdinfoMark {
    index: Option<Index>,
    mount_id: Option<usize>,
    flags: FanotifyMarkFlags,
    mask: FanotifyEventMask,
    ignored_mask: FanotifyEventMask,
}

struct FanotifyInner {
    id: usize,
    generation: AtomicUsize,
    fd_count: AtomicUsize,
    report_dfid_name: bool,
    unprivileged: bool,
    marks: SpinLock<Vec<FanotifyFdinfoMark>>,
    pending: SpinLock<Vec<FanotifyEvent>>,
    responses: SpinLock<Vec<(i32, Arc<FanotifyPermission>)>>,
    waiter: SpinLock<WaitQueue<Event>>,
    epoll_notifier: Arc<EpollNotifier>,
}

impl FanotifyInner {
    const RESPONSE_ALLOW: u32 = 0x01;
    const RESPONSE_DENY: u32 = 0x02;

    fn new(report_dfid_name: bool, unprivileged: bool) -> Self {
        Self {
            id: NEXT_FANOTIFY_ID.fetch_add(1, Ordering::Relaxed),
            generation: AtomicUsize::new(0),
            fd_count: AtomicUsize::new(0),
            report_dfid_name,
            unprivileged,
            marks: SpinLock::new(Vec::new(), "FanotifyInner::marks"),
            pending: SpinLock::new(Vec::new(), "FanotifyInner::pending"),
            responses: SpinLock::new(Vec::new(), "FanotifyInner::responses"),
            waiter: SpinLock::new(WaitQueue::new(), "FanotifyInner::waiter"),
            epoll_notifier: Arc::new(EpollNotifier::new()),
        }
    }

    fn flush_marks(&self) {
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

    fn pop_event(&self, blocked: bool) -> SysResult<FanotifyEvent> {
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

    fn respond(&self, fd: i32, response: u32) -> SysResult<()> {
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

    fn is_ignore_mark(flags: FanotifyMarkFlags) -> bool {
        flags.intersects(FanotifyMarkFlags::FAN_MARK_IGNORE | FanotifyMarkFlags::FAN_MARK_IGNORED_MASK)
    }

    fn add_mark(
        &self,
        index: Option<Index>,
        mount_id: Option<usize>,
        flags: FanotifyMarkFlags,
        mask: FanotifyEventMask,
    ) {
        let flags = Self::fdinfo_mark_flags(flags);
        let scope_flags = Self::fdinfo_mark_scope_flags(flags);
        let is_ignore_mark = Self::is_ignore_mark(flags);
        let mut marks = self.marks.lock();
        if let Some(mark) = marks.iter_mut().find(|mark| {
            mark.index == index && mark.mount_id == mount_id && Self::fdinfo_mark_scope_flags(mark.flags) == scope_flags
        }) {
            if is_ignore_mark {
                mark.ignored_mask.insert(mask);
            } else {
                mark.mask.insert(mask);
            }
            return;
        }

        marks.push(FanotifyFdinfoMark {
            index,
            mount_id,
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

    fn remove_mark(
        &self,
        index: Option<Index>,
        mount_id: Option<usize>,
        flags: FanotifyMarkFlags,
        mask: FanotifyEventMask,
    ) {
        let flags = Self::fdinfo_mark_flags(flags);
        let scope_flags = Self::fdinfo_mark_scope_flags(flags);
        let is_ignore_mark = Self::is_ignore_mark(flags);
        let mut marks = self.marks.lock();
        for mark in marks.iter_mut().filter(|mark| {
            mark.index == index && mark.mount_id == mount_id && Self::fdinfo_mark_scope_flags(mark.flags) == scope_flags
        }) {
            if is_ignore_mark {
                mark.ignored_mask.remove(mask);
            } else {
                mark.mask.remove(mask);
            }
        }
        marks.retain(|mark| !mark.mask.is_empty() || !mark.ignored_mask.is_empty());
    }

    fn fdinfo(&self) -> String {
        let mut content = String::new();
        for mark in self.marks.lock().iter() {
            let index = mark.index.unwrap_or(Index { sno: 0, ino: 0 });
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
}

impl FanotifyListener for FanotifyInner {
    fn fanotify_id(&self) -> usize {
        self.id
    }

    fn fanotify_generation(&self) -> usize {
        self.generation.load(Ordering::Acquire)
    }

    fn add_fanotify_mark(
        &self,
        index: Option<Index>,
        mount_id: Option<usize>,
        flags: FanotifyMarkFlags,
        mask: FanotifyEventMask,
    ) {
        self.add_mark(index, mount_id, flags, mask);
    }

    fn remove_fanotify_mark(
        &self,
        index: Option<Index>,
        mount_id: Option<usize>,
        flags: FanotifyMarkFlags,
        mask: FanotifyEventMask,
    ) {
        self.remove_mark(index, mount_id, flags, mask);
    }

    fn queue_fanotify_event(&self, mask: FanotifyEventMask, file: Option<Arc<dyn FileOps>>) {
        self.queue_event(FanotifyEvent {
            mask,
            file,
            pid: current::pid() as i32,
            permission: None,
        });
    }

    fn queue_fanotify_permission(&self, mask: FanotifyEventMask, file: Arc<dyn FileOps>) -> SysResult<()> {
        self.queue_permission(mask, file)
    }
}

pub struct FanotifyFile {
    inner: Arc<FanotifyInner>,
    flags: SpinLock<FileFlags>,
}

impl FanotifyFile {
    const IO_BYTES: usize = FanotifyEventMetadata::SIZE;
    /// struct fanotify_response {
    ///    __i32 fd,
    ///    __u32 response;
    /// };
    const RESPONSE_SIZE: usize = core::mem::size_of::<i32>() + core::mem::size_of::<u32>();

    pub fn new(blocked: bool, report_dfid_name: bool, unprivileged: bool) -> Self {
        Self {
            inner: Arc::new(FanotifyInner::new(report_dfid_name, unprivileged)),
            flags: SpinLock::new(
                FileFlags {
                    readable: true,
                    writable: true,
                    blocked,
                    append: false,
                    direct: false,
                },
                "FanotifyFile::flags",
            ),
        }
    }

    pub fn listener(&self) -> Arc<dyn FanotifyListener> {
        self.inner.clone()
    }

    pub fn listener_id(&self) -> usize {
        self.inner.id
    }

    pub fn listener_generation(&self) -> usize {
        self.inner.fanotify_generation()
    }

    pub fn flush_marks(&self) {
        self.inner.flush_marks();
    }

    pub fn unprivileged(&self) -> bool {
        self.inner.unprivileged
    }

    pub fn report_dfid_name(&self) -> bool {
        self.inner.report_dfid_name
    }

    fn blocked(&self) -> bool {
        self.flags.lock().blocked
    }

    fn validate_io_len(len: usize) -> SysResult<()> {
        if len >= Self::IO_BYTES {
            Ok(())
        } else {
            Err(Errno::EINVAL)
        }
    }
}

impl FileOps for FanotifyFile {
    fn read(&self, buf: &mut [u8]) -> SysResult<usize> {
        Self::validate_io_len(buf.len())?;
        let event = self.inner.pop_event(self.blocked())?;
        let mut written = event.write_to(&self.inner, buf)?;

        loop {
            let next_len = {
                let pending = self.inner.pending.lock();
                let Some(event) = pending.first() else {
                    break;
                };
                event.encoded_len(&self.inner)
            };
            if written + next_len > buf.len() {
                break;
            }

            let event = self.inner.pending.lock().remove(0);
            written += event.write_to(&self.inner, &mut buf[written..written + next_len])?;
        }

        Ok(written)
    }

    fn read_to_user(&self, ubuf: &UAddrSpaceBuffer) -> SysResult<usize> {
        Self::validate_io_len(ubuf.length())?;
        let mut buf = Vec::new();
        buf.resize(ubuf.length(), 0);
        let len = self.read(&mut buf)?;
        copy_to_user::buffer(ubuf.uaddr(), &buf[..len])?;
        Ok(len)
    }

    fn write(&self, buf: &[u8]) -> SysResult<usize> {
        if buf.len() < Self::RESPONSE_SIZE {
            return Err(Errno::EINVAL);
        }

        let fd = i32::from_ne_bytes(buf[0..4].try_into().unwrap());
        let response = u32::from_ne_bytes(buf[4..8].try_into().unwrap());
        self.inner.respond(fd, response)?;
        Ok(Self::RESPONSE_SIZE)
    }

    fn write_from_user(&self, ubuf: &UAddrSpaceBuffer) -> SysResult<usize> {
        if ubuf.length() < Self::RESPONSE_SIZE {
            return Err(Errno::EINVAL);
        }

        let mut buf = [0u8; Self::RESPONSE_SIZE];
        copy_from_user::slice(ubuf.uaddr(), &mut buf)?;
        self.write(&buf)
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
        if !event.contains(FileEvent::READ_READY) {
            return Ok(None);
        }

        if !self.inner.pending.lock().is_empty() {
            return Ok(Some(FileEvent::READ_READY));
        }

        Ok(None)
    }

    fn wait_event(&self, waker: usize, event: FileEvent) -> SysResult<Option<FileEvent>> {
        if !event.contains(FileEvent::READ_READY) {
            return Ok(None);
        }

        if let Some(ready) = self.poll_event(event)? {
            return Ok(Some(ready));
        }

        self.inner.waiter.lock().wait(
            current::task().clone(),
            Event::Poll {
                event: FileEvent::READ_READY,
                waker,
            },
        );

        Ok(None)
    }

    fn wait_event_cancel(&self) {
        self.inner.waiter.lock().remove(current::task());
    }

    fn epoll_notifier(&self) -> Option<Arc<EpollNotifier>> {
        Some(self.inner.epoll_notifier.clone())
    }

    fn set_flags(&self, flags: FileFlags) {
        *self.flags.lock() = FileFlags {
            readable: true,
            writable: true,
            blocked: flags.blocked,
            append: false,
            direct: false,
        };
    }

    fn on_fd_install(&self) -> SysResult<()> {
        self.inner.fd_count.fetch_add(1, Ordering::AcqRel);
        Ok(())
    }

    fn on_fd_remove(&self) {
        if self.inner.fd_count.fetch_sub(1, Ordering::AcqRel) == 1 {
            self.inner.flush_marks();
        }
    }

    fn fdinfo(&self) -> Option<String> {
        Some(self.inner.fdinfo())
    }

    fn type_name(&self) -> &'static str {
        "fanotify"
    }
}
