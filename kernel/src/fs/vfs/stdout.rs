use core::u32;

use alloc::boxed::Box;
use alloc::sync::Arc;
use spin::Lazy;

use crate::fs::file::File;
use crate::fs::{inode, Dentry, Inode, LockedInode};
use crate::fs::file::FileFlags;
use crate::kernel::errno::Errno;
use crate::platform;

pub struct StdoutInode;

impl Inode for StdoutInode {
    fn get_ino(&self) -> u32 {
        0
    }

    fn get_sno(&self) -> u32 {
        panic!("StdoutInode does not belong to a filesystem")
    }
    
    fn readat(&mut self, _buf: &mut [u8], _offset: usize) -> Result<usize, Errno> {
        Ok(0)
    }

    fn writeat(&mut self, buf: &[u8], _offset: usize) -> Result<usize, Errno> {
        buf.iter().for_each(|&byte| {
            platform::putchar(byte);
        });

        Ok(buf.len())
    }

    fn size(&self) -> Result<usize, Errno> {
        Ok(0)
    }
}

static STDOUT: Lazy<Arc<File>> = Lazy::new(|| {
    Arc::new(
        File::new(
            &Arc::new(
                Dentry::new_noparent(
                    "stdout", 
                    &Arc::new(LockedInode::new(&inode::Index { sno: u32::MAX, ino: u32::MAX }, Box::new(StdoutInode)))
                )
            ),
            FileFlags::dontcare()
        )
    )
});

pub fn stdout() -> Arc<File> {
    STDOUT.clone()
}
 