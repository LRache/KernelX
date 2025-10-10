use core::u32;

use alloc::sync::Arc;
use spin::Lazy;

use crate::kernel::errno::{Errno, SysResult};
use crate::fs::file::File;
use crate::fs::{Dentry, Inode};
use crate::fs::file::FileFlags;
use crate::platform;

pub struct StdoutInode;

impl Inode for StdoutInode {
    fn get_ino(&self) -> u32 {
        0
    }

    fn get_sno(&self) -> u32 {
        // panic!("StdoutInode does not belong to a filesystem")
        u32::MAX
    }

    fn type_name(&self) -> &'static str {
        "stdout"
    }
    
    fn readat(&self, _buf: &mut [u8], _offset: usize) -> Result<usize, Errno> {
        Ok(0)
    }

    fn writeat(&self, buf: &[u8], _offset: usize) -> Result<usize, Errno> {
        buf.iter().for_each(|&byte| {
            platform::putchar(byte);
        });

        Ok(buf.len())
    }

    fn size(&self) -> SysResult<u64> {
        Ok(0)
    }
}

static STDOUT: Lazy<Arc<File>> = Lazy::new(|| {
    Arc::new(
        File::new(
            &Arc::new(
                Dentry::new_noparent(
                    "stdout", 
                    &(Arc::new(StdoutInode {}) as Arc<dyn Inode>)
                )
            ),
            FileFlags::dontcare()
        )
    )
});

pub fn stdout() -> Arc<File> {
    STDOUT.clone()
}
 