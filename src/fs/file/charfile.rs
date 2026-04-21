use alloc::sync::Arc;
use core::sync::atomic::{AtomicUsize, Ordering};

use crate::driver::CharDriverOps;
use crate::fs::file::{FileFlags, FileOps};
use crate::fs::inode::release_bsd_flock;
use crate::fs::{Dentry, InodeOps};
use crate::kernel::errno::SysResult;
use crate::kernel::event::{FileEvent, PollEventSet};
use crate::kernel::mm::AddrSpace;
use crate::kernel::uapi::FileStat;

pub struct CharFile {
    driver: Arc<dyn CharDriverOps>,
    inode: Arc<dyn InodeOps>,
    dentry: Option<Arc<Dentry>>,
    readable: bool,
    writable: bool,
    blocked: bool,
    fd_refs: AtomicUsize,
}

impl CharFile {
    pub fn new(
        driver: Arc<dyn CharDriverOps>,
        inode: Arc<dyn InodeOps>,
        dentry: Option<Arc<Dentry>>,
        flags: FileFlags,
    ) -> Self {
        CharFile {
            driver,
            inode,
            dentry,
            readable: flags.readable,
            writable: flags.writable,
            blocked: flags.blocked,
            fd_refs: AtomicUsize::new(0),
        }
    }

    fn release_bsd_flock_if_last_fd(&self) {
        let previous = self.fd_refs.fetch_sub(1, Ordering::AcqRel);
        debug_assert!(previous > 0, "CharFile::fd_refs underflow");
        if previous == 1 {
            release_bsd_flock(&self.inode, self.flock_owner_id());
        }
    }
}

impl FileOps for CharFile {
    fn read(&self, buf: &mut [u8]) -> SysResult<usize> {
        self.driver.read(buf, self.blocked)
    }

    fn write(&self, buf: &[u8]) -> SysResult<usize> {
        self.driver.write(buf)
    }

    fn flags(&self) -> FileFlags {
        FileFlags {
            readable: self.readable,
            writable: self.writable,
            blocked: self.blocked,
            append: false,
            direct: false,
        }
    }

    fn fstat(&self) -> SysResult<FileStat> {
        self.inode.fstat()
    }

    fn fsync(&self) -> SysResult<()> {
        Ok(())
    }

    fn get_dentry(&self) -> Option<&Arc<Dentry>> {
        self.dentry.as_ref()
    }

    fn get_inode(&self) -> Option<&Arc<dyn InodeOps>> {
        Some(&self.inode)
    }

    fn ioctl(&self, request: usize, arg: usize, addrspace: &AddrSpace) -> SysResult<usize> {
        self.driver.ioctl(request, arg, addrspace)
    }

    fn wait_event(&self, waker: usize, event: PollEventSet) -> SysResult<Option<FileEvent>> {
        self.driver.wait_event(waker, event)
    }

    fn wait_event_cancel(&self) {
        self.driver.wait_event_cancel();
    }

    fn type_name(&self) -> &'static str {
        "CharFile"
    }

    fn on_fd_install(&self) -> SysResult<()> {
        if self.writable {
            self.inode.begin_write_open()?;
        }
        self.fd_refs.fetch_add(1, Ordering::Relaxed);
        Ok(())
    }

    fn on_fd_remove(&self) {
        if self.writable {
            self.inode.end_write_open();
        }
        self.release_bsd_flock_if_last_fd();
    }
}

unsafe impl Send for CharFile {}
unsafe impl Sync for CharFile {}
