use crate::fs::inode::Inode;
use crate::kernel::errno::{Errno, SysResult};
use super::ffi;

pub struct Ext4Inode {
    pub ino: u32,
    sno: u32,
    inode_handler: usize,
    destroyed: bool
}

impl Ext4Inode {
    pub fn new(ino: u32, sno: u32, fs_handler: usize) -> Result<Self, Errno> {
        Ok(Self {
            ino,
            sno,
            inode_handler: ffi::get_inode_handler(fs_handler, ino)?,
            destroyed: false
        })
    }
}

impl Inode for Ext4Inode {
    fn get_ino(&self) -> u32 {
        self.ino
    }

    fn get_sno(&self) -> u32 {
        self.sno
    }

    fn type_name(&self) -> &'static str {
        "ext4"
    }

    fn readat(&mut self, buf: &mut [u8], offset: usize) -> Result<usize, Errno> {
        Ok(ffi::inode_readat(self.inode_handler, buf, offset)? as usize)
    }

    fn writeat(&mut self, buf: &[u8], offset: usize) -> Result<usize, Errno> {
        ffi::inode_writeat(self.inode_handler, buf, offset)
    }

    fn lookup(&mut self, name: &str) -> SysResult<u32> {
        ffi::inode_lookup(self.inode_handler, name)
    }

    fn mkdir(&mut self, name: &str) -> SysResult<()> {
        ffi::inode_mkdir(self.inode_handler, name).map(|_| ())
    }

    fn create(&mut self, name: &str, _flags: crate::fs::file::FileFlags) -> SysResult<()> {
        ffi::create_inode(self.inode_handler, name, 0644).map(|_| ())
    }

    fn size(&self) -> Result<usize, Errno> {
        ffi::inode_get_size(self.inode_handler)
    }

    fn destroy(&mut self) -> SysResult<()> {
        if !self.destroyed {
            ffi::put_inode_handler(self.inode_handler)?;
            self.destroyed = true;
        }
        Ok(())
    }
}

impl Drop for Ext4Inode {
    fn drop(&mut self) {
        if !self.destroyed {
            let _ = self.destroy();
            self.destroyed = true;
        }
    }
}
