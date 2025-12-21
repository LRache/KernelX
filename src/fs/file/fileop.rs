use alloc::sync::Arc;
use downcast_rs::{DowncastSync, impl_downcast};

use crate::fs::file::FileFlags;
use crate::kernel::event::{FileEvent, PollEventSet};
use crate::kernel::errno::{Errno, SysResult};
use crate::kernel::mm::AddrSpace;
use crate::kernel::uapi::FileStat;
use crate::fs::{Dentry, InodeOps};

pub enum SeekWhence {
    BEG,
    CUR,
    END,
}

pub trait FileOps: DowncastSync {
    fn read(&self, buf: &mut [u8]) -> SysResult<usize>;
    fn pread(&self, buf: &mut [u8], offset: usize) -> SysResult<usize>;
    fn write(&self, buf: &[u8]) -> SysResult<usize>;
    fn pwrite(&self, buf: &[u8], offset: usize) -> SysResult<usize>;
    // fn read_from_user(&self, ubuf: usize, length: usize, addrspace: &AddrSpace) -> SysResult<usize> {
    //     let mut kbuf = [0u8; 1024];
    //     let mut total_read = 0;
    //     let mut remaining = length;
    //     while remaining > 0 {
    //         let to_read = core::cmp::min(remaining, kbuf.len());
    //         addrspace.copy_from_user_buffer(ubuf + total_read, &mut kbuf[..to_read])?;
    //         let n = self.read(&mut kbuf[..to_read])?;
    //         if n == 0 {
    //             break;
    //         }
    //         total_read += n;
    //         remaining -= n;
    //     }
    //     Ok(total_read)
    // }
    // fn write_to_user(&self, ubuf: usize, length: usize, addrspace: &AddrSpace) -> SysResult<usize> {
    //     let mut kbuf = [0u8; 1024];
    //     let mut total_written = 0;
    //     let mut remaining = length;
    //     while remaining > 0 {
    //         let to_write = core::cmp::min(remaining, kbuf.len());
    //         addrspace.copy_from_user_buffer(ubuf + total_written, &mut kbuf[..to_write])?;
    //         let n = self.write(&kbuf[..to_write])?;
    //         if n == 0 {
    //             break;
    //         }
    //         total_written += n;
    //         remaining -= n;
    //     }
    //     Ok(total_written)
    // }

    fn readable(&self) -> bool;
    fn writable(&self) -> bool;
    
    fn seek(&self, offset: isize, whence: SeekWhence) -> SysResult<usize>;
    fn ioctl(&self, _request: usize, _arg: usize, _addrspace: &AddrSpace) -> SysResult<usize> {
        Err(Errno::ENOSYS)
    }
    fn fstat(&self) -> SysResult<FileStat>;
    fn fsync(&self) -> SysResult<()>;
    
    fn get_inode(&self) -> Option<&Arc<dyn InodeOps>>;
    fn get_dentry(&self) -> Option<&Arc<Dentry>>;

    fn wait_event(&self, _waker: usize, _event: PollEventSet) -> SysResult<Option<FileEvent>> {
        Ok(None)
    }
    fn wait_event_cancel(&self) {}

    fn set_flags(&self, flags: FileFlags) {
        let _ = flags;
    }

    fn type_name(&self) -> &'static str {
        "unknown"
    }
}

impl_downcast!(sync FileOps);
