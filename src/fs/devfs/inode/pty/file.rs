use alloc::sync::Arc;
use core::sync::atomic::{AtomicUsize, Ordering};

use num_enum::TryFromPrimitive;

use crate::driver::char::tty::TtyIoctlResult;
use crate::fs::Dentry;
use crate::fs::file::{FileFlags, FileOps};
use crate::fs::inode::{Inode, release_bsd_flock};
use crate::kernel::errno::{Errno, SysResult};
use crate::kernel::event::{EpollNotifier, Event, FileEvent};
use crate::kernel::mm::AddrSpace;
use crate::kernel::scheduler::current;
use crate::kernel::uapi::FileStat;
use crate::klib::SpinLock;

use super::inner::PtyInner;

pub(super) struct PtmxFile {
    inner: Arc<PtyInner>,
    inode: Arc<Inode>,
    dentry: Option<Arc<Dentry>>,
    flags: SpinLock<FileFlags>,
    fd_refs: AtomicUsize,
}

impl PtmxFile {
    pub(super) fn new(inner: Arc<PtyInner>, inode: Arc<Inode>, dentry: Option<Arc<Dentry>>, flags: FileFlags) -> Self {
        Self {
            inner,
            inode,
            dentry,
            flags: SpinLock::new(flags, "PtmxFile::flags"),
            fd_refs: AtomicUsize::new(0),
        }
    }

    fn release_bsd_flock_if_last_fd(&self) {
        let previous = self.fd_refs.fetch_sub(1, Ordering::AcqRel);
        debug_assert!(previous > 0, "PtmxFile::fd_refs underflow");
        if previous == 1 {
            release_bsd_flock(&self.inode, self.flock_owner_id());
        }
    }
}

impl Drop for PtmxFile {
    fn drop(&mut self) {
        self.inner.hangup_master();
    }
}

impl FileOps for PtmxFile {
    fn read(&self, buf: &mut [u8]) -> SysResult<usize> {
        self.inner.read_master(buf, self.block())
    }

    fn write(&self, buf: &[u8]) -> SysResult<usize> {
        self.inner.write_master(buf)
    }

    fn flags(&self) -> FileFlags {
        *self.flags.lock()
    }

    fn set_flags(&self, flags: FileFlags) {
        self.flags.lock().blocked = flags.blocked;
    }

    fn fstat(&self) -> SysResult<FileStat> {
        self.inode.fstat()
    }

    fn fsync(&self) -> SysResult<()> {
        Ok(())
    }

    fn get_inode(&self) -> Option<&Arc<Inode>> {
        Some(&self.inode)
    }

    fn get_dentry(&self) -> Option<&Arc<Dentry>> {
        self.dentry.as_ref()
    }

    fn ioctl(&self, request: usize, arg: usize, addrspace: &AddrSpace) -> SysResult<usize> {
        match self.inner.tty.ioctl(request, arg, addrspace)? {
            TtyIoctlResult::Handled(ret) => return Ok(ret),
            TtyIoctlResult::FlushInput(ret) => {
                self.inner.flush_input();
                return Ok(ret);
            }
            TtyIoctlResult::Unsupported => {}
        }

        #[repr(usize)]
        #[derive(Debug, Clone, Copy, TryFromPrimitive)]
        enum PtyIoctlReq {
            TIOCGPTN = 0x80045430,
            TIOCSPTLCK = 0x40045431,
            TIOCGPTLCK = 0x80045439,
        }

        match PtyIoctlReq::try_from(request).map_err(|_| Errno::ENOTTY)? {
            PtyIoctlReq::TIOCGPTN => {
                addrspace.copy_to_user(arg, self.inner.id as u32)?;
                Ok(0)
            }
            PtyIoctlReq::TIOCSPTLCK => {
                let locked = addrspace.copy_from_user::<i32>(arg)? != 0;
                self.inner.set_locked(locked);
                Ok(0)
            }
            PtyIoctlReq::TIOCGPTLCK => {
                let locked = if self.inner.locked() { 1i32 } else { 0i32 };
                addrspace.copy_to_user(arg, locked)?;
                Ok(0)
            }
        }
    }

    fn poll_event(&self, event: FileEvent) -> SysResult<Option<FileEvent>> {
        Ok(self.inner.master_poll_event(event))
    }

    fn wait_event(&self, waker: usize, event: FileEvent) -> SysResult<Option<FileEvent>> {
        if let Some(ready) = self.poll_event(event)? {
            return Ok(Some(ready));
        }

        if event.contains(FileEvent::READ_READY) {
            self.inner.master_waiters.lock().wait_current(Event::Poll {
                event: FileEvent::READ_READY,
                waker,
            });
        }

        Ok(None)
    }

    fn wait_event_cancel(&self) {
        self.inner.master_waiters.lock().remove(current::task());
    }

    fn epoll_notifier(&self) -> Option<Arc<EpollNotifier>> {
        Some(self.inner.master_epoll.clone())
    }

    fn on_fd_install(&self) -> SysResult<()> {
        if self.writable() {
            self.inode.begin_write_open()?;
        }
        self.inner.master_open();
        self.fd_refs.fetch_add(1, Ordering::Relaxed);
        Ok(())
    }

    fn on_fd_remove(&self) {
        if self.writable() {
            self.inode.end_write_open();
        }
        self.inner.master_close();
        self.release_bsd_flock_if_last_fd();
    }

    fn type_name(&self) -> &'static str {
        "PtmxFile"
    }
}

pub struct PtsFile {
    inner: Arc<PtyInner>,
    inode: Arc<Inode>,
    dentry: Option<Arc<Dentry>>,
    flags: SpinLock<FileFlags>,
    fd_refs: AtomicUsize,
}

impl PtsFile {
    pub fn new(inner: Arc<PtyInner>, inode: Arc<Inode>, dentry: Option<Arc<Dentry>>, flags: FileFlags) -> Self {
        Self {
            inner,
            inode,
            dentry,
            flags: SpinLock::new(flags, "PtsFile::flags"),
            fd_refs: AtomicUsize::new(0),
        }
    }

    fn release_bsd_flock_if_last_fd(&self) {
        let previous = self.fd_refs.fetch_sub(1, Ordering::AcqRel);
        debug_assert!(previous > 0, "PtsFile::fd_refs underflow");
        if previous == 1 {
            release_bsd_flock(&self.inode, self.flock_owner_id());
        }
    }
}

impl FileOps for PtsFile {
    fn read(&self, buf: &mut [u8]) -> SysResult<usize> {
        self.inner.read_slave(buf, self.block())
    }

    fn write(&self, buf: &[u8]) -> SysResult<usize> {
        self.inner.write_slave(buf)
    }

    fn flags(&self) -> FileFlags {
        *self.flags.lock()
    }

    fn set_flags(&self, flags: FileFlags) {
        self.flags.lock().blocked = flags.blocked;
    }

    fn fstat(&self) -> SysResult<FileStat> {
        self.inode.fstat()
    }

    fn fsync(&self) -> SysResult<()> {
        Ok(())
    }

    fn get_inode(&self) -> Option<&Arc<Inode>> {
        Some(&self.inode)
    }

    fn get_dentry(&self) -> Option<&Arc<Dentry>> {
        self.dentry.as_ref()
    }

    fn ioctl(&self, request: usize, arg: usize, addrspace: &AddrSpace) -> SysResult<usize> {
        match self.inner.tty.ioctl(request, arg, addrspace)? {
            TtyIoctlResult::Handled(ret) => Ok(ret),
            TtyIoctlResult::FlushInput(ret) => {
                self.inner.flush_input();
                Ok(ret)
            }
            TtyIoctlResult::Unsupported => Err(Errno::ENOTTY),
        }
    }

    fn poll_event(&self, event: FileEvent) -> SysResult<Option<FileEvent>> {
        Ok(self.inner.slave_poll_event(event))
    }

    fn wait_event(&self, waker: usize, event: FileEvent) -> SysResult<Option<FileEvent>> {
        if let Some(ready) = self.poll_event(event)? {
            return Ok(Some(ready));
        }

        if event.contains(FileEvent::READ_READY) {
            self.inner.slave_waiters.lock().wait_current(Event::Poll {
                event: FileEvent::READ_READY,
                waker,
            });
        }

        Ok(None)
    }

    fn wait_event_cancel(&self) {
        self.inner.slave_waiters.lock().remove(current::task());
    }

    fn epoll_notifier(&self) -> Option<Arc<EpollNotifier>> {
        Some(self.inner.slave_epoll.clone())
    }

    fn on_fd_install(&self) -> SysResult<()> {
        self.inner.slave_open()?;
        if self.writable() {
            if let Err(err) = self.inode.begin_write_open() {
                self.inner.slave_close();
                return Err(err);
            }
        }
        self.fd_refs.fetch_add(1, Ordering::Relaxed);
        Ok(())
    }

    fn on_fd_remove(&self) {
        if self.writable() {
            self.inode.end_write_open();
        }
        self.inner.slave_close();
        self.release_bsd_flock_if_last_fd();
    }

    fn type_name(&self) -> &'static str {
        "PtsFile"
    }
}
