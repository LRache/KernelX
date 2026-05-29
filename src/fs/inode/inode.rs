use alloc::sync::Arc;
use core::time::Duration;
use downcast_rs::{DowncastSync, impl_downcast};

use crate::fs::file::{DirResult, FileFlags, FileOps};
use crate::fs::{Dentry, Perm};
use crate::kernel::errno::{Errno, SysResult};
use crate::kernel::event::Fanotify;
use crate::kernel::mm::ubuf::UAddrSpaceBuffer;
use crate::kernel::mm::{AddrSpace, PhysPageFrame};
use crate::kernel::uapi::{FileFallocateFlags, FileSealFlags, FileStat, Uid};
use crate::klib::SpinLock;

use super::bsd_flock::BsdFlockState;
use super::posix_flock::PosixFlockState;
use super::{FileType, Mode, Owner};

pub struct InodeLockState {
    pub(crate) bsd: BsdFlockState,
    pub(crate) posix: PosixFlockState,
    writer_count: u32,
    exec_count: u32,
}

impl InodeLockState {
    pub fn new() -> Self {
        Self {
            bsd: BsdFlockState::new(),
            posix: PosixFlockState::new(),
            writer_count: 0,
            exec_count: 0,
        }
    }

    pub fn writer_count(&self) -> u32 {
        self.writer_count
    }

    pub fn exec_count(&self) -> u32 {
        self.exec_count
    }

    pub fn increment_writer_count(&mut self) {
        self.writer_count = self
            .writer_count
            .checked_add(1)
            .expect("InodeLockState::writer_count overflow");
    }

    pub fn decrement_writer_count(&mut self) {
        debug_assert!(self.writer_count > 0, "InodeLockState::writer_count underflow");
        self.writer_count -= 1;
    }

    pub fn increment_exec_count(&mut self) {
        self.exec_count = self
            .exec_count
            .checked_add(1)
            .expect("InodeLockState::exec_count overflow");
    }

    pub fn decrement_exec_count(&mut self) {
        debug_assert!(self.exec_count > 0, "InodeLockState::exec_count underflow");
        self.exec_count -= 1;
    }
}

pub fn release_bsd_flock(inode: &Arc<dyn InodeOps>, owner: usize) {
    let Some(lock_state) = inode.lock_state() else {
        return;
    };

    let mut lock_state = lock_state.lock();
    if lock_state.bsd.remove_owner(owner) {
        lock_state.bsd.wake_all();
    }
}

pub trait InodeSealOps: Send + Sync {
    fn init_seals(&self, seals: FileSealFlags) -> SysResult<()>;

    fn seals(&self) -> SysResult<FileSealFlags>;

    fn add_seals(&self, seals: FileSealFlags) -> SysResult<()>;

    fn begin_shared_mmap(&self, _writable: bool) {}

    fn update_shared_mmap_writable(&self, _old_writable: bool, _new_writable: bool) {}

    fn end_shared_mmap(&self, _writable: bool) {}
}

pub trait InodeOps: DowncastSync {
    fn get_ino(&self) -> u32;

    fn type_name(&self) -> &'static str;

    /// Number of extra `Arc<Self>` references that the filesystem keeps
    /// even when the inode is otherwise idle. The inode cache itself is
    /// accounted for separately.
    fn filesystem_refcount_bias(&self) -> usize {
        0
    }

    fn lock_state(&self) -> Option<&SpinLock<InodeLockState>> {
        None
    }

    fn fanotify(&self) -> Option<Arc<Fanotify>> {
        None
    }

    fn ensure_fanotify(&self) -> Option<Arc<Fanotify>> {
        self.fanotify()
    }

    fn as_seal_ops(&self) -> Option<&dyn InodeSealOps> {
        None
    }

    fn begin_write_open(&self) -> SysResult<()> {
        let Some(lock_state) = self.lock_state() else {
            return Ok(());
        };

        let mut lock_state = lock_state.lock();
        if lock_state.exec_count() > 0 {
            return Err(Errno::ETXTBSY);
        }
        lock_state.increment_writer_count();
        Ok(())
    }

    fn end_write_open(&self) {
        let Some(lock_state) = self.lock_state() else {
            return;
        };

        lock_state.lock().decrement_writer_count();
    }

    fn begin_exec(&self) -> SysResult<()> {
        let Some(lock_state) = self.lock_state() else {
            return Ok(());
        };

        let mut lock_state = lock_state.lock();
        if lock_state.writer_count() > 0 {
            return Err(Errno::ETXTBSY);
        }
        lock_state.increment_exec_count();
        Ok(())
    }

    fn increment_exec_count(&self) {
        let Some(lock_state) = self.lock_state() else {
            return;
        };

        lock_state.lock().increment_exec_count();
    }

    fn end_exec(&self) {
        let Some(lock_state) = self.lock_state() else {
            return;
        };

        lock_state.lock().decrement_exec_count();
    }

    fn create(&self, _name: &str, _mode: Mode, _owner: Owner) -> SysResult<Arc<dyn InodeOps>> {
        Err(Errno::EOPNOTSUPP)
    }

    fn mknod(&self, name: &str, mode: Mode, owner: Owner, _dev: u64) -> SysResult<Arc<dyn InodeOps>> {
        if (mode & Mode::S_IFMT) == Mode::S_IFIFO {
            self.create(name, mode, owner)
        } else {
            Err(Errno::EOPNOTSUPP)
        }
    }

    fn link(&self, name: &str, target: &Arc<dyn InodeOps>) -> SysResult<()> {
        let _ = name;
        let _ = target;
        Err(Errno::EOPNOTSUPP)
    }

    fn unlink(&self, _name: &str) -> SysResult<()> {
        Err(Errno::EOPNOTSUPP)
    }

    fn symlink(&self, target: &str) -> SysResult<()> {
        let _ = target;
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

    fn rename(&self, _old_name: &str, _new_parent: &Arc<dyn InodeOps>, _new_name: &str) -> SysResult<()> {
        Err(Errno::EOPNOTSUPP)
    }

    fn readlink(&self, buf: &mut [u8]) -> SysResult<Option<usize>> {
        let _ = buf;
        Ok(None)
    }

    fn follow_magic_link(&self) -> SysResult<Option<Arc<Dentry>>> {
        Ok(None)
    }

    fn size(&self) -> SysResult<u64>;

    fn mmap_shared_page(&self, file_page_index: usize) -> SysResult<Option<Arc<PhysPageFrame>>> {
        let offset = file_page_index.checked_mul(crate::arch::PGSIZE).ok_or(Errno::EFBIG)?;
        let file_size = usize::try_from(self.size()?).map_err(|_| Errno::EFBIG)?;
        if offset >= file_size {
            return Ok(None);
        }

        let len = core::cmp::min(file_size - offset, crate::arch::PGSIZE);
        let frame = PhysPageFrame::alloc_zeroed();
        let read_len = self.readat(&mut frame.slice()[..len], offset, false)?;
        if read_len < len {
            frame.slice()[read_len..len].fill(0);
        }
        Ok(Some(Arc::new(frame)))
    }

    fn writeback_mmap_shared_page(&self, file_page_index: usize, frame: &PhysPageFrame) -> SysResult<()> {
        let offset = file_page_index.checked_mul(crate::arch::PGSIZE).ok_or(Errno::EFBIG)?;
        let file_size = usize::try_from(self.size()?).map_err(|_| Errno::EFBIG)?;
        if offset >= file_size {
            return Ok(());
        }

        let len = core::cmp::min(file_size - offset, crate::arch::PGSIZE);
        self.writeat(&frame.slice()[..len], offset)?;
        Ok(())
    }

    fn release_mmap_shared_page(&self, _file_page_index: usize) {}

    fn mode(&self) -> SysResult<Mode> {
        Ok(Mode::empty())
    }

    fn check_perm(&self, perm: &Perm) -> SysResult<bool> {
        let owner = self.owner()?;
        Ok(self.mode()?.check_perm(perm, owner.0, owner.1))
    }

    fn chmod(&self, _mode: Mode) -> SysResult<()> {
        Err(Errno::EOPNOTSUPP)
    }

    fn owner(&self) -> SysResult<(Uid, Uid)> {
        Ok((0, 0))
    }

    fn chown(&self, uid: Option<Uid>, gid: Option<Uid>) -> SysResult<()> {
        let _ = uid;
        let _ = gid;
        Err(Errno::EOPNOTSUPP)
    }

    fn inode_type(&self) -> SysResult<FileType> {
        self.mode().map(|inode| inode.into())
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

    fn update_atime(&self, time: &Duration) -> SysResult<()> {
        let _ = time;
        Ok(())
    }

    fn update_mtime(&self, time: &Duration) -> SysResult<()> {
        let _ = time;
        Ok(())
    }

    fn update_ctime(&self, time: &Duration) -> SysResult<()> {
        let _ = time;
        Ok(())
    }

    fn open_file(self: Arc<Self>, dentry: Option<Arc<Dentry>>, flags: FileFlags) -> SysResult<Arc<dyn FileOps>> {
        Ok(self.wrap_file(dentry, flags))
    }

    fn wrap_file(self: Arc<Self>, dentry: Option<Arc<Dentry>>, flags: FileFlags) -> Arc<dyn FileOps>;
}

impl_downcast!(sync InodeOps);
