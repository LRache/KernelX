mod attr;
mod dir;
mod file;

use alloc::boxed::Box;
pub use attr::FileAttr;
pub use dir::{DirEntry, DirLookupResult, DirReader};

use core::marker::PhantomData;

use crate::{SystemHal, ffi::*};

/// Inode type.
#[repr(u8)]
#[derive(PartialEq, Default, Eq, Clone, Copy, Debug)]
pub enum InodeType {
    #[default]
    Unknown = 0,
    Fifo = 1,
    CharacterDevice = 2,
    Directory = 4,
    BlockDevice = 6,
    RegularFile = 8,
    Symlink = 10,
    Socket = 12,
}
impl From<u8> for InodeType {
    fn from(value: u8) -> Self {
        match value {
            1 => InodeType::Fifo,
            2 => InodeType::CharacterDevice,
            4 => InodeType::Directory,
            6 => InodeType::BlockDevice,
            8 => InodeType::RegularFile,
            10 => InodeType::Symlink,
            12 => InodeType::Socket,
            _ => InodeType::Unknown,
        }
    }
}

#[repr(transparent)]
pub struct InodeRef<Hal: SystemHal> {
    pub(crate) inner: Box<ext4_inode_ref>,
    _phantom: PhantomData<Hal>,
}
impl<Hal: SystemHal> InodeRef<Hal> {
    pub(crate) fn new(inner: ext4_inode_ref) -> Self {
        Self {
            inner: Box::new(inner),
            _phantom: PhantomData,
        }
    }

    pub fn ino(&self) -> u32 {
        self.inner.index
    }

    pub(crate) fn superblock(&self) -> &ext4_sblock {
        unsafe { &(*self.inner.fs).sb }
    }
    pub(crate) fn superblock_mut(&mut self) -> &mut ext4_sblock {
        unsafe { &mut (*self.inner.fs).sb }
    }

    pub(crate) fn mark_dirty(&mut self) {
        self.inner.dirty = true;
    }

    pub(crate) fn inc_nlink(&mut self) {
        unsafe {
            ext4_fs_inode_links_count_inc(self.inner.as_mut());
        }
        self.mark_dirty();
    }
    pub(crate) fn dec_nlink(&mut self) {
        self.set_nlink(self.nlink() - 1);
        self.mark_dirty();
    }

    pub(crate) fn set_nlink(&mut self, nlink: u16) {
        self.raw_inode_mut().links_count = u16::to_le(nlink);
        self.mark_dirty();
    }

    pub(crate) fn raw_inode(&self) -> &ext4_inode {
        unsafe { &*self.inner.inode }
    }
    pub(crate) fn raw_inode_mut(&mut self) -> &mut ext4_inode {
        unsafe { &mut *self.inner.inode }
    }
}

impl<Hal: SystemHal> Drop for InodeRef<Hal> {
    fn drop(&mut self) {
        let ret = unsafe { ext4_fs_put_inode_ref(self.inner.as_mut()) };
        if ret != 0 {
            panic!("ext4_fs_put_inode_ref failed: {}", ret);
        }
    }
}
