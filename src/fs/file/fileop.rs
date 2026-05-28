use alloc::boxed::Box;
use alloc::string::String;
use alloc::sync::Arc;
use alloc::vec::Vec;
use downcast_rs::{DowncastSync, impl_downcast};

use crate::fs::file::FileFlags;
use crate::fs::{Dentry, InodeOps, vfs};
use crate::kernel::errno::{Errno, SysResult};
use crate::kernel::event::{EpollNotifier, FileEvent};
use crate::kernel::mm::maparea::Area;
use crate::kernel::mm::ubuf::UAddrSpaceBuffer;
use crate::kernel::mm::{AddrSpace, MapPerm};
use crate::kernel::uapi::{FileStat, Statfs};

#[derive(Debug, Clone, Copy)]
pub enum SeekWhence {
    BEG,
    CUR,
    END,
}

#[derive(Debug, Clone, Copy)]
pub struct FileMmapRequest {
    pub shared: bool,
    pub perm: MapPerm,
    pub offset: usize,
    pub length: usize,
}

pub trait FileOps: DowncastSync {
    fn read(&self, buf: &mut [u8]) -> SysResult<usize>;
    fn write(&self, buf: &[u8]) -> SysResult<usize>;

    fn read_to_user(&self, ubuf: &UAddrSpaceBuffer) -> SysResult<usize> {
        let mut total_read = 0;
        for kbuf in ubuf.iter_mut() {
            let kbuf = kbuf?;
            let n = self.read(kbuf)?;
            total_read += n;
            if n < kbuf.len() {
                return Ok(total_read);
            }
        }
        Ok(total_read)
    }

    fn write_from_user(&self, ubuf: &UAddrSpaceBuffer) -> SysResult<usize> {
        let mut total_written = 0;
        for kbuf in ubuf.iter() {
            let kbuf = kbuf?;
            let n = self.write(kbuf)?;
            total_written += n;
            if n < kbuf.len() {
                return Ok(total_written);
            }
        }

        Ok(total_written)
    }

    fn flags(&self) -> FileFlags;

    fn readable(&self) -> bool {
        self.flags().readable
    }

    fn writable(&self) -> bool {
        self.flags().writable
    }

    fn block(&self) -> bool {
        self.flags().blocked
    }

    fn ioctl(&self, _request: usize, _arg: usize, _addrspace: &AddrSpace) -> SysResult<usize> {
        Err(Errno::ENOTTY)
    }
    fn fstat(&self) -> SysResult<FileStat>;

    fn fstatfs(&self) -> SysResult<Statfs> {
        let sno = self.get_dentry().ok_or(Errno::EINVAL)?.sno();
        vfs::statfs(sno)
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

    fn mmap_area(self: Arc<Self>, request: FileMmapRequest) -> SysResult<Box<dyn Area>> {
        let _ = request;
        Err(Errno::ENODEV)
    }

    fn poll_event(&self, event: FileEvent) -> SysResult<Option<FileEvent>> {
        let _ = event;
        Ok(None)
    }

    fn wait_event(&self, _waker: usize, event: FileEvent) -> SysResult<Option<FileEvent>> {
        self.poll_event(event)
    }
    fn wait_event_cancel(&self) {}

    fn epoll_notifier(&self) -> Option<Arc<EpollNotifier>> {
        None
    }

    fn epoll_notifiers(&self) -> Option<Vec<Arc<EpollNotifier>>> {
        self.epoll_notifier().map(|notifier| {
            let mut notifiers = Vec::new();
            notifiers.push(notifier);
            notifiers
        })
    }

    fn set_flags(&self, flags: FileFlags) {
        let _ = flags;
    }

    fn type_name(&self) -> &'static str {
        "unknown"
    }

    fn flock_owner_id(&self) -> usize {
        self as *const Self as *const () as usize
    }

    fn on_fd_install(&self) -> SysResult<()> {
        Ok(())
    }

    fn on_fd_remove(&self) {}

    fn fdinfo(&self) -> Option<String> {
        None
    }
}

impl_downcast!(sync FileOps);
