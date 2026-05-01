use alloc::sync::Arc;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicUsize, Ordering};

use crate::fs::file::{FileFlags, FileOps};
use crate::fs::inode::{FanotifyEventMask, FanotifyListener};
use crate::fs::{Dentry, InodeOps, Mode};
use crate::kernel::errno::{Errno, SysResult};
use crate::kernel::event::{Event, FileEvent, WaitQueue};
use crate::kernel::mm::ubuf::UAddrSpaceBuffer;
use crate::kernel::scheduler::current;
use crate::kernel::scheduler::current::copy_to_user;
use crate::kernel::task::fdtable::FDFlags;
use crate::kernel::uapi::FileStat;
use crate::klib::SpinLock;

static NEXT_FANOTIFY_ID: AtomicUsize = AtomicUsize::new(1);
const FANOTIFY_METADATA_VERSION: u8 = 3;
const FAN_NOFD: i32 = -1;

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
    const SIZE: usize = core::mem::size_of::<Self>();

    fn new(mask: FanotifyEventMask, fd: i32, pid: i32) -> Self {
        Self {
            event_len: Self::SIZE as u32,
            vers: FANOTIFY_METADATA_VERSION,
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

#[derive(Clone)]
struct FanotifyEvent {
    mask: FanotifyEventMask,
    file: Option<Arc<dyn FileOps>>,
    pid: i32,
}

impl FanotifyEvent {
    fn metadata(self) -> SysResult<FanotifyEventMetadata> {
        let fd = if let Some(file) = self.file {
            current::fdtable().lock().push(file, FDFlags::empty())? as i32
        } else {
            FAN_NOFD
        };
        Ok(FanotifyEventMetadata::new(self.mask, fd, self.pid))
    }
}

struct FanotifyInner {
    id: usize,
    generation: AtomicUsize,
    pending: SpinLock<Vec<FanotifyEvent>>,
    waiter: SpinLock<WaitQueue<Event>>,
}

impl FanotifyInner {
    fn new() -> Self {
        Self {
            id: NEXT_FANOTIFY_ID.fetch_add(1, Ordering::Relaxed),
            generation: AtomicUsize::new(0),
            pending: SpinLock::new(Vec::new(), "FanotifyInner::pending"),
            waiter: SpinLock::new(WaitQueue::new(), "FanotifyInner::waiter"),
        }
    }

    fn flush_marks(&self) {
        self.generation.fetch_add(1, Ordering::AcqRel);
        self.pending.lock().clear();
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
}

impl FanotifyListener for FanotifyInner {
    fn fanotify_id(&self) -> usize {
        self.id
    }

    fn fanotify_generation(&self) -> usize {
        self.generation.load(Ordering::Acquire)
    }

    fn queue_fanotify_event(&self, mask: FanotifyEventMask, file: Option<Arc<dyn FileOps>>) {
        self.pending.lock().push(FanotifyEvent {
            mask,
            file,
            pid: current::pid() as i32,
        });
        self.waiter.lock().wake_all(|event| event);
    }
}

pub struct FanotifyFile {
    inner: Arc<FanotifyInner>,
    flags: SpinLock<FileFlags>,
}

impl FanotifyFile {
    const IO_BYTES: usize = FanotifyEventMetadata::SIZE;

    pub fn new(blocked: bool) -> Self {
        Self {
            inner: Arc::new(FanotifyInner::new()),
            flags: SpinLock::new(
                FileFlags {
                    readable: true,
                    writable: false,
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
        event.metadata()?.write_to(&mut buf[..Self::IO_BYTES]);
        let mut written = Self::IO_BYTES;

        while written + Self::IO_BYTES <= buf.len() {
            let event = {
                let mut pending = self.inner.pending.lock();
                if pending.is_empty() {
                    break;
                }
                pending.remove(0)
            };
            event.metadata()?.write_to(&mut buf[written..written + Self::IO_BYTES]);
            written += Self::IO_BYTES;
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

    fn wait_event(&self, waker: usize, event: FileEvent) -> SysResult<Option<FileEvent>> {
        if !event.contains(FileEvent::READ_READY) {
            return Ok(None);
        }

        if !self.inner.pending.lock().is_empty() {
            return Ok(Some(FileEvent::READ_READY));
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

    fn set_flags(&self, flags: FileFlags) {
        *self.flags.lock() = FileFlags {
            readable: true,
            writable: false,
            blocked: flags.blocked,
            append: false,
            direct: false,
        };
    }

    fn type_name(&self) -> &'static str {
        "fanotify"
    }
}
