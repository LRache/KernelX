use alloc::sync::Weak;

use super::superblock::MemoryFileSystemSuperBlock;
use crate::fs::inode::{Inode, InodeNumber};
use crate::kernel::errno::Errno;

#[derive(Clone)]
pub struct MemoryFileSystemInode {
    pub ino: u32,
    pub superblock: Weak<MemoryFileSystemSuperBlock>,

    pub start: *mut u8,
    pub size : usize,
}

unsafe impl Send for MemoryFileSystemInode {}
unsafe impl Sync for MemoryFileSystemInode {}

impl Inode for MemoryFileSystemInode {
    fn get_ino(&self) -> u32 {
        self.ino
    }

    fn get_fsno(&self) -> u32 {
        self.superblock.upgrade().expect("MemoryFileSystemInode: superblock is gone").get_fsno()
    }

    fn readat(&mut self, buf: &mut [u8], offset: usize) -> Result<usize, Errno>  {
        if self.size <= offset {
            return Ok(0);
        }
        let len = core::cmp::min(self.size - offset, buf.len());
        buf[..len].copy_from_slice(unsafe {
            core::slice::from_raw_parts(self.start.add(offset), len)
        });
        Ok(len)
    }

    fn writeat(&mut self, _buf: &[u8], _offset: usize) -> Result<usize, Errno> {
        Ok(0)
    }

    fn lookup(&mut self, name: &str) -> Result<InodeNumber, Errno> {
        let superblock = self.superblock.upgrade().ok_or(Errno::ENOENT)?;
        superblock.lookup(self.ino, name).ok_or(Errno::ENOENT)
    }
}