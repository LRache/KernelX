use crate::fs::inode::Inode;
use crate::fs::inode::InodeNumber;
use crate::kernel::errno::Errno;
use super::ffi;

pub struct Ext4Inode {
    pub ino: u32,
    fsno: usize,
    inode_handler: usize
}

impl Ext4Inode {
    pub fn new(ino: u32, fsno: usize, fs_handler: usize) -> Result<Self, Errno> {
        Ok(Self {
            ino,
            fsno,
            inode_handler: ffi::get_inode_handler(fs_handler, ino)?
        })
    }
}

impl Inode for Ext4Inode {
    fn get_ino(&self) -> InodeNumber {
        self.ino as usize
    }

    fn get_fsno(&self) -> usize {
        self.fsno
    }

    fn readat(&mut self, buf: &mut [u8], offset: usize) -> Result<usize, Errno> {
        Ok(ffi::inode_readat(self.inode_handler, buf, offset)? as usize)
    }

    fn writeat(&mut self, buf: &[u8], offset: usize) -> Result<usize, Errno> {
        ffi::inode_writeat(self.inode_handler, buf, offset)
    }

    fn lookup(&self, name: &str) -> Result<InodeNumber, Errno> {
        Ok(ffi::inode_lookup(self.inode_handler, name)? as InodeNumber)
    }

    fn size(&self) -> Result<usize, Errno> {
        ffi::inode_get_size(self.inode_handler)
    }
}

impl Drop for Ext4Inode {
    fn drop(&mut self) {
        let _ = ffi::put_inode_handler(self.inode_handler);
    }
}
