use alloc::sync::Arc;
use core::time::Duration;

use crate::fs::file::{DirResult, FileFlags, FileOps};
use crate::fs::inode::{Inode as VfsInode, Mode, Owner};
use crate::fs::{Dentry, FileType, InodeOps};
use crate::kernel::errno::SysResult;
use crate::kernel::mm::ubuf::UAddrSpaceBuffer;
use crate::kernel::mm::{AddrSpace, PhysPageFrame};
use crate::kernel::uapi::{FileFallocateFlags, FileStat, Uid};

use super::super::superblock::StaticFsInfo;
use super::MemInodeOps;

impl<T: StaticFsInfo> InodeOps for Arc<dyn MemInodeOps<T>> {
    fn get_ino(&self) -> u32 {
        self.as_ref().get_ino()
    }

    fn type_name(&self) -> &'static str {
        self.as_ref().type_name()
    }

    fn create(&self, name: &str, mode: Mode, owner: Owner) -> SysResult<Self> {
        self.as_ref().create(name, mode, owner)
    }

    fn mknod(&self, name: &str, mode: Mode, owner: Owner, dev: u64) -> SysResult<Self> {
        self.as_ref().mknod(name, mode, owner, dev)
    }

    fn link(&self, name: &str, target: &Self) -> SysResult<()> {
        self.as_ref().link(name, target.as_ref())
    }

    fn unlink(&self, name: &str) -> SysResult<()> {
        self.as_ref().unlink(name)
    }

    fn symlink(&self, target: &str) -> SysResult<()> {
        self.as_ref().symlink(target)
    }

    fn readat(&self, buf: &mut [u8], offset: usize, direct: bool) -> SysResult<usize> {
        self.as_ref().readat(buf, offset, direct)
    }

    fn writeat(&self, buf: &[u8], offset: usize) -> SysResult<usize> {
        self.as_ref().writeat(buf, offset)
    }

    fn read_to_user(&self, ubuf: &UAddrSpaceBuffer, offset: usize, direct: bool) -> SysResult<usize> {
        self.as_ref().read_to_user(ubuf, offset, direct)
    }

    fn write_from_user(&self, ubuf: &UAddrSpaceBuffer, offset: usize, direct: bool) -> SysResult<usize> {
        self.as_ref().write_from_user(ubuf, offset, direct)
    }

    fn get_dent(&self, index: usize) -> SysResult<Option<(DirResult, usize)>> {
        self.as_ref().get_dent(index)
    }

    fn lookup(&self, name: &str) -> SysResult<u32> {
        self.as_ref().lookup(name)
    }

    fn rename(&self, old_name: &str, new_parent: &Self, new_name: &str) -> SysResult<()> {
        self.as_ref().rename(old_name, new_parent.as_ref(), new_name)
    }

    fn readlink(&self, buf: &mut [u8]) -> SysResult<Option<usize>> {
        self.as_ref().readlink(buf)
    }

    fn follow_magic_link(&self) -> SysResult<Option<Arc<Dentry>>> {
        self.as_ref().follow_magic_link()
    }

    fn size(&self) -> SysResult<u64> {
        self.as_ref().size()
    }

    fn mmap_shared_page(&self, file_page_index: usize) -> SysResult<Option<Arc<PhysPageFrame>>> {
        self.as_ref().mmap_shared_page(file_page_index)
    }

    fn writeback_mmap_shared_page(&self, file_page_index: usize, frame: &PhysPageFrame) -> SysResult<()> {
        self.as_ref().writeback_mmap_shared_page(file_page_index, frame)
    }

    fn release_mmap_shared_page(&self, file_page_index: usize) {
        self.as_ref().release_mmap_shared_page(file_page_index)
    }

    fn mode(&self) -> SysResult<Mode> {
        self.as_ref().mode()
    }

    fn chmod(&self, mode: Mode) -> SysResult<()> {
        self.as_ref().chmod(mode)
    }

    fn owner(&self) -> SysResult<(Uid, Uid)> {
        self.as_ref().owner()
    }

    fn chown(&self, uid: Option<Uid>, gid: Option<Uid>) -> SysResult<()> {
        self.as_ref().chown(uid, gid)
    }

    fn inode_type(&self) -> SysResult<FileType> {
        self.as_ref().inode_type()
    }

    fn sync(&self) -> SysResult<()> {
        self.as_ref().sync()
    }

    fn ioctl(&self, request: usize, arg: usize, addrspace: &AddrSpace) -> SysResult<usize> {
        self.as_ref().ioctl(request, arg, addrspace)
    }

    fn fstat(&self) -> SysResult<FileStat> {
        self.as_ref().fstat()
    }

    fn truncate(&self, new_size: u64) -> SysResult<()> {
        self.as_ref().truncate(new_size)
    }

    fn fallocate(&self, flags: FileFallocateFlags, offset: u64, len: u64) -> SysResult<()> {
        self.as_ref().fallocate(flags, offset, len)
    }

    fn update_atime(&self, time: &Duration) -> SysResult<()> {
        self.as_ref().update_atime(time)
    }

    fn update_mtime(&self, time: &Duration) -> SysResult<()> {
        self.as_ref().update_mtime(time)
    }

    fn update_ctime(&self, time: &Duration) -> SysResult<()> {
        self.as_ref().update_ctime(time)
    }

    fn update_mtime_ctime(&self, time: &Duration) -> SysResult<()> {
        self.as_ref().update_mtime_ctime(time)
    }

    fn open_file(
        &self,
        inode: Arc<VfsInode>,
        dentry: Option<Arc<Dentry>>,
        flags: FileFlags,
    ) -> SysResult<Arc<dyn FileOps>> {
        self.as_ref().open_file(inode, dentry, flags)
    }

    fn wrap_file(&self, inode: Arc<VfsInode>, dentry: Option<Arc<Dentry>>, flags: FileFlags) -> Arc<dyn FileOps> {
        self.as_ref().wrap_file(inode, dentry, flags)
    }
}
