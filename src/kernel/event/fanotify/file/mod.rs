mod event;
mod inner;
mod permission;

use alloc::string::String;
use alloc::sync::Arc;
use alloc::vec::Vec;
use core::sync::atomic::Ordering;

use crate::fs::file::{FileFlags, FileOps};
use crate::fs::{Dentry, Inode, Mode};
use crate::kernel::errno::{Errno, SysResult};
use crate::kernel::event::{EpollNotifier, Event, FileEvent};
use crate::kernel::mm::ubuf::UAddrSpaceBuffer;
use crate::kernel::scheduler::current;
use crate::kernel::scheduler::current::{copy_from_user, copy_to_user};
use crate::kernel::uapi::FileStat;
use crate::klib::SpinLock;

use self::event::FanotifyEvent;
pub use self::inner::FanotifyListener;

pub struct FanotifyFile {
    inner: Arc<FanotifyListener>,
    flags: SpinLock<FileFlags>,
}

impl FanotifyFile {
    const IO_BYTES: usize = FanotifyEvent::MIN_READ_SIZE;
    /// struct fanotify_response {
    ///    __i32 fd,
    ///    __u32 response;
    /// };
    const RESPONSE_SIZE: usize = core::mem::size_of::<i32>() + core::mem::size_of::<u32>();

    pub fn new(blocked: bool, report_dfid_name: bool, unprivileged: bool) -> Self {
        Self {
            inner: Arc::new(FanotifyListener::new(report_dfid_name, unprivileged)),
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

    pub fn listener(&self) -> Arc<FanotifyListener> {
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
        let mut written = match event.write_to(&self.inner, buf) {
            Ok(written) => written,
            Err(err) => {
                self.inner.pending.lock().insert(0, event);
                return Err(err);
            }
        };

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
            match event.write_to(&self.inner, &mut buf[written..written + next_len]) {
                Ok(event_len) => written += event_len,
                Err(_) => {
                    self.inner.pending.lock().insert(0, event);
                    break;
                }
            }
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

    fn get_inode(&self) -> Option<&Arc<Inode>> {
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

        let pending = self.inner.pending.lock();
        if !pending.is_empty() {
            return Ok(Some(FileEvent::READ_READY));
        }

        self.inner.waiter.lock().wait_pending(
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
