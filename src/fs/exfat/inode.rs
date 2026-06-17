use alloc::sync::Arc;

use crate::fs::file::{DirResult, FileFlags, FileOps, RandomAccessFile};
use crate::fs::inode::{InodeOps, Mode, Owner};
use crate::fs::{Dentry, FileType};
use crate::kernel::errno::{Errno, SysResult};
use crate::kernel::uapi::{FileStat, Uid};
use crate::klib::SleepLock;

use super::superblock::SuperBlockInner;

pub struct Inode {
    ino: u32,
    superblock: Arc<SleepLock<SuperBlockInner>>,
}

impl Inode {
    pub fn new(ino: u32, superblock: Arc<SleepLock<SuperBlockInner>>) -> Self {
        Self { ino, superblock }
    }
}

impl InodeOps for Inode {
    fn get_ino(&self) -> u32 {
        self.ino
    }

    fn type_name(&self) -> &'static str {
        "exfat"
    }

    fn create(&self, _name: &str, _mode: Mode, _owner: Owner) -> SysResult<Arc<dyn InodeOps>> {
        Err(Errno::EROFS)
    }

    fn mknod(&self, _name: &str, _mode: Mode, _owner: Owner, _dev: u64) -> SysResult<Arc<dyn InodeOps>> {
        Err(Errno::EROFS)
    }

    fn link(&self, _name: &str, _target: &Arc<dyn InodeOps>) -> SysResult<()> {
        Err(Errno::EROFS)
    }

    fn unlink(&self, _name: &str) -> SysResult<()> {
        Err(Errno::EROFS)
    }

    fn symlink(&self, _target: &str) -> SysResult<()> {
        Err(Errno::EROFS)
    }

    fn rename(&self, _old_name: &str, _new_parent: &Arc<dyn InodeOps>, _new_name: &str) -> SysResult<()> {
        Err(Errno::EROFS)
    }

    fn readat(&self, buf: &mut [u8], offset: usize, _direct: bool) -> SysResult<usize> {
        self.superblock.lock().read_inode(self.ino, buf, offset)
    }

    fn writeat(&self, _buf: &[u8], _offset: usize) -> SysResult<usize> {
        Err(Errno::EROFS)
    }

    fn get_dent(&self, index: usize) -> SysResult<Option<(DirResult, usize)>> {
        self.superblock.lock().get_dent(self.ino, index)
    }

    fn lookup(&self, name: &str) -> SysResult<u32> {
        self.superblock.lock().lookup(self.ino, name)
    }

    fn size(&self) -> SysResult<u64> {
        self.superblock.lock().inode_size(self.ino)
    }

    fn mode(&self) -> SysResult<Mode> {
        self.superblock.lock().inode_mode(self.ino)
    }

    fn chmod(&self, _mode: Mode) -> SysResult<()> {
        Err(Errno::EROFS)
    }

    fn owner(&self) -> SysResult<(Uid, Uid)> {
        Ok((0, 0))
    }

    fn chown(&self, _uid: Option<Uid>, _gid: Option<Uid>) -> SysResult<()> {
        Err(Errno::EROFS)
    }

    fn inode_type(&self) -> SysResult<FileType> {
        self.superblock.lock().inode_type(self.ino)
    }

    fn fstat(&self) -> SysResult<FileStat> {
        self.superblock.lock().inode_stat(self.ino)
    }

    fn truncate(&self, _new_size: u64) -> SysResult<()> {
        Err(Errno::EROFS)
    }

    fn wrap_file(self: Arc<Self>, dentry: Option<Arc<Dentry>>, flags: FileFlags) -> Arc<dyn FileOps> {
        Arc::new(RandomAccessFile::new(self, dentry.unwrap(), flags))
    }
}
