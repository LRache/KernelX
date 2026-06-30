use alloc::string::String;
use alloc::sync::Arc;
use core::time::Duration;

use crate::fs::file::{DirResult, FileFlags, FileOps};
use crate::fs::inode::{Inode as VfsInode, Mode, Owner};
use crate::fs::{Dentry, FileType, InodeOps};
use crate::kernel::errno::{Errno, SysResult};
use crate::kernel::mm::ubuf::UAddrSpaceBuffer;
use crate::kernel::mm::{AddrSpace, PhysPageFrame};
use crate::kernel::uapi::{FileFallocateFlags, FileStat, Uid};

use super::super::superblock::StaticFsInfo;
use super::RegularInode;

pub trait MemInodeOps<T: StaticFsInfo>: Send + Sync + 'static {
    fn as_regular(&self) -> Option<&RegularInode<T>> {
        None
    }

    fn get_ino(&self) -> u32;

    fn type_name(&self) -> &'static str;

    fn create(&self, _name: &str, _mode: Mode, _owner: Owner) -> SysResult<Arc<dyn MemInodeOps<T>>> {
        Err(Errno::EOPNOTSUPP)
    }

    fn mknod(&self, name: &str, mode: Mode, owner: Owner, _dev: u64) -> SysResult<Arc<dyn MemInodeOps<T>>> {
        if (mode & Mode::S_IFMT) == Mode::S_IFIFO {
            self.create(name, mode, owner)
        } else {
            Err(Errno::EOPNOTSUPP)
        }
    }

    fn add_child(&self, _name: String, _child: Arc<dyn MemInodeOps<T>>) -> SysResult<()> {
        Err(Errno::ENOTDIR)
    }

    fn link(&self, _name: &str, _target: &dyn MemInodeOps<T>) -> SysResult<()> {
        Err(Errno::EOPNOTSUPP)
    }

    fn unlink(&self, _name: &str) -> SysResult<()> {
        Err(Errno::EOPNOTSUPP)
    }

    fn symlink(&self, _target: &str) -> SysResult<()> {
        Err(Errno::EOPNOTSUPP)
    }

    fn readat(&self, _buf: &mut [u8], _offset: usize, _direct: bool) -> SysResult<usize> {
        unimplemented!()
    }

    fn writeat(&self, _buf: &[u8], _offset: usize) -> SysResult<usize> {
        unimplemented!()
    }

    fn read_to_user(&self, ubuf: &UAddrSpaceBuffer, offset: usize, direct: bool) -> SysResult<usize> {
        let mut total_read = 0;
        let mut current_offset = offset;
        for kbuf in ubuf.iter_mut() {
            let kbuf = kbuf?;
            let n = self.readat(kbuf, current_offset, direct)?;
            total_read += n;
            current_offset += n;
            if n < kbuf.len() {
                return Ok(total_read);
            }
        }
        Ok(total_read)
    }

    fn write_from_user(&self, ubuf: &UAddrSpaceBuffer, offset: usize, _direct: bool) -> SysResult<usize> {
        let mut total_written = 0;
        let mut current_offset = offset;
        for kbuf in ubuf.iter() {
            let kbuf = kbuf?;
            let n = self.writeat(kbuf, current_offset)?;
            total_written += n;
            current_offset += n;
            if n < kbuf.len() {
                return Ok(total_written);
            }
        }
        Ok(total_written)
    }

    fn get_dent(&self, _index: usize) -> SysResult<Option<(DirResult, usize)>> {
        Err(Errno::ENOTDIR)
    }

    fn lookup(&self, _name: &str) -> SysResult<u32> {
        Err(Errno::ENOTDIR)
    }

    fn rename(&self, _old_name: &str, _new_parent: &dyn MemInodeOps<T>, _new_name: &str) -> SysResult<()> {
        Err(Errno::EOPNOTSUPP)
    }

    fn readlink(&self, _buf: &mut [u8]) -> SysResult<Option<usize>> {
        Ok(None)
    }

    fn follow_magic_link(&self) -> SysResult<Option<Arc<Dentry>>> {
        Ok(None)
    }

    fn size(&self) -> SysResult<u64>;

    fn mmap_shared_page(&self, _file_page_index: usize) -> SysResult<Option<Arc<PhysPageFrame>>> {
        Ok(None)
    }

    fn writeback_mmap_shared_page(&self, _file_page_index: usize, _frame: &PhysPageFrame) -> SysResult<()> {
        Ok(())
    }

    fn release_mmap_shared_page(&self, _file_page_index: usize) {}

    fn mode(&self) -> SysResult<Mode> {
        Ok(Mode::empty())
    }

    fn chmod(&self, _mode: Mode) -> SysResult<()> {
        Err(Errno::EOPNOTSUPP)
    }

    fn owner(&self) -> SysResult<(Uid, Uid)> {
        Ok((0, 0))
    }

    fn chown(&self, _uid: Option<Uid>, _gid: Option<Uid>) -> SysResult<()> {
        Err(Errno::EOPNOTSUPP)
    }

    fn inode_type(&self) -> SysResult<FileType> {
        self.mode().map(|mode| mode.into())
    }

    fn sync(&self) -> SysResult<()> {
        Ok(())
    }

    fn ioctl(&self, _request: usize, _arg: usize, _addrspace: &AddrSpace) -> SysResult<usize> {
        Err(Errno::ENOTTY)
    }

    fn fstat(&self) -> SysResult<FileStat> {
        let mut kstat = FileStat::default();
        kstat.st_ino = self.get_ino() as u64;
        kstat.st_size = self.size()? as i64;
        kstat.st_mode = self.mode()?.bits() as u32;

        Ok(kstat)
    }

    fn truncate(&self, _new_size: u64) -> SysResult<()> {
        Err(Errno::EOPNOTSUPP)
    }

    fn fallocate(&self, flags: FileFallocateFlags, offset: u64, len: u64) -> SysResult<()> {
        if !flags.is_empty() {
            return Err(Errno::EOPNOTSUPP);
        }

        let new_size = offset.checked_add(len).ok_or(Errno::EFBIG)?;
        if new_size <= self.size()? {
            return Ok(());
        }

        self.truncate(new_size)
    }

    fn update_atime(&self, _time: &Duration) -> SysResult<()> {
        Ok(())
    }

    fn update_mtime(&self, _time: &Duration) -> SysResult<()> {
        Ok(())
    }

    fn update_ctime(&self, _time: &Duration) -> SysResult<()> {
        Ok(())
    }

    fn update_mtime_ctime(&self, time: &Duration) -> SysResult<()> {
        self.update_mtime(time)?;
        self.update_ctime(time)
    }

    fn open_file(
        &self,
        inode: Arc<VfsInode>,
        dentry: Option<Arc<Dentry>>,
        flags: FileFlags,
    ) -> SysResult<Arc<dyn FileOps>> {
        Ok(self.wrap_file(inode, dentry, flags))
    }

    fn wrap_file(&self, inode: Arc<VfsInode>, dentry: Option<Arc<Dentry>>, flags: FileFlags) -> Arc<dyn FileOps>;
}

impl<T: StaticFsInfo, I: InodeOps> MemInodeOps<T> for I {
    fn get_ino(&self) -> u32 {
        InodeOps::get_ino(self)
    }

    fn type_name(&self) -> &'static str {
        InodeOps::type_name(self)
    }

    fn unlink(&self, name: &str) -> SysResult<()> {
        InodeOps::unlink(self, name)
    }

    fn symlink(&self, target: &str) -> SysResult<()> {
        InodeOps::symlink(self, target)
    }

    fn readat(&self, buf: &mut [u8], offset: usize, direct: bool) -> SysResult<usize> {
        InodeOps::readat(self, buf, offset, direct)
    }

    fn writeat(&self, buf: &[u8], offset: usize) -> SysResult<usize> {
        InodeOps::writeat(self, buf, offset)
    }

    fn read_to_user(&self, ubuf: &UAddrSpaceBuffer, offset: usize, direct: bool) -> SysResult<usize> {
        InodeOps::read_to_user(self, ubuf, offset, direct)
    }

    fn write_from_user(&self, ubuf: &UAddrSpaceBuffer, offset: usize, direct: bool) -> SysResult<usize> {
        InodeOps::write_from_user(self, ubuf, offset, direct)
    }

    fn get_dent(&self, index: usize) -> SysResult<Option<(DirResult, usize)>> {
        InodeOps::get_dent(self, index)
    }

    fn lookup(&self, name: &str) -> SysResult<u32> {
        InodeOps::lookup(self, name)
    }

    fn readlink(&self, buf: &mut [u8]) -> SysResult<Option<usize>> {
        InodeOps::readlink(self, buf)
    }

    fn follow_magic_link(&self) -> SysResult<Option<Arc<Dentry>>> {
        InodeOps::follow_magic_link(self)
    }

    fn size(&self) -> SysResult<u64> {
        InodeOps::size(self)
    }

    fn mmap_shared_page(&self, file_page_index: usize) -> SysResult<Option<Arc<PhysPageFrame>>> {
        InodeOps::mmap_shared_page(self, file_page_index)
    }

    fn writeback_mmap_shared_page(&self, file_page_index: usize, frame: &PhysPageFrame) -> SysResult<()> {
        InodeOps::writeback_mmap_shared_page(self, file_page_index, frame)
    }

    fn release_mmap_shared_page(&self, file_page_index: usize) {
        InodeOps::release_mmap_shared_page(self, file_page_index)
    }

    fn mode(&self) -> SysResult<Mode> {
        InodeOps::mode(self)
    }

    fn chmod(&self, mode: Mode) -> SysResult<()> {
        InodeOps::chmod(self, mode)
    }

    fn owner(&self) -> SysResult<(Uid, Uid)> {
        InodeOps::owner(self)
    }

    fn chown(&self, uid: Option<Uid>, gid: Option<Uid>) -> SysResult<()> {
        InodeOps::chown(self, uid, gid)
    }

    fn inode_type(&self) -> SysResult<FileType> {
        InodeOps::inode_type(self)
    }

    fn sync(&self) -> SysResult<()> {
        InodeOps::sync(self)
    }

    fn ioctl(&self, request: usize, arg: usize, addrspace: &AddrSpace) -> SysResult<usize> {
        InodeOps::ioctl(self, request, arg, addrspace)
    }

    fn fstat(&self) -> SysResult<FileStat> {
        InodeOps::fstat(self)
    }

    fn truncate(&self, new_size: u64) -> SysResult<()> {
        InodeOps::truncate(self, new_size)
    }

    fn fallocate(&self, flags: FileFallocateFlags, offset: u64, len: u64) -> SysResult<()> {
        InodeOps::fallocate(self, flags, offset, len)
    }

    fn update_atime(&self, time: &Duration) -> SysResult<()> {
        InodeOps::update_atime(self, time)
    }

    fn update_mtime(&self, time: &Duration) -> SysResult<()> {
        InodeOps::update_mtime(self, time)
    }

    fn update_ctime(&self, time: &Duration) -> SysResult<()> {
        InodeOps::update_ctime(self, time)
    }

    fn update_mtime_ctime(&self, time: &Duration) -> SysResult<()> {
        InodeOps::update_mtime_ctime(self, time)
    }

    fn open_file(
        &self,
        inode: Arc<VfsInode>,
        dentry: Option<Arc<Dentry>>,
        flags: FileFlags,
    ) -> SysResult<Arc<dyn FileOps>> {
        InodeOps::open_file(self, inode, dentry, flags)
    }

    fn wrap_file(&self, inode: Arc<VfsInode>, dentry: Option<Arc<Dentry>>, flags: FileFlags) -> Arc<dyn FileOps> {
        InodeOps::wrap_file(self, inode, dentry, flags)
    }
}
