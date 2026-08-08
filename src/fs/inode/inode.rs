use alloc::sync::{Arc, Weak};
use core::ops::Deref;
use core::time::Duration;
use downcast_rs::{DowncastSync, impl_downcast};

use crate::driver::{BlockDriverOps, CharDriverOps};
use crate::fs::file::{DirResult, FileFlags, FileOps};
use crate::fs::{Dentry, Perm};
use crate::kernel::config;
use crate::kernel::errno::{Errno, SysResult};
#[cfg(feature = "fanotify")]
use crate::kernel::event::Fanotify;
use crate::kernel::ipc::pipe::PipeInner;
use crate::kernel::mm::swappable::{FileMapping, SharedFilePage};
use crate::kernel::mm::ubuf::UAddrSpaceBuffer;
use crate::kernel::mm::{AddrSpace, PhysPageFrame};
use crate::kernel::uapi::{FileFallocateFlags, FileSealFlags, FileStat, Uid};
use crate::klib::{LazyInitedCell, SleepRwLockOnStack, SpinLock};

use super::bsd_flock::BsdFlockState;
use super::posix_flock::PosixFlockState;
use super::{FileType, Mode, Owner};

pub type Inode = dyn VfsInodeOps;

#[derive(Default)]
pub struct InodeLifecycleState {
    pub tmpfile_linkable: bool,
}

pub struct VfsInode<T: InodeOps> {
    inner: T,
    file_mapping: Option<Arc<FileMapping>>,
    // Per canonical inode: reads and writes may proceed together, while
    // truncate and namespace removal require exclusive lifetime ownership.
    lifecycle: SleepRwLockOnStack<InodeLifecycleState>,
    lock_state: SpinLock<InodeLockState>,
    seal_state: SpinLock<InodeSealState>,
    fifo_pipe: LazyInitedCell<Arc<PipeInner>>,
    #[cfg(feature = "fanotify")]
    fanotify: LazyInitedCell<Arc<Fanotify>>,
}

impl<T: InodeOps> VfsInode<T> {
    pub fn new(mut inner: T) -> Arc<Self> {
        Arc::new_cyclic(|weak: &Weak<Self>| {
            let inode: Weak<Inode> = weak.clone();
            let file_mapping = inner.supports_file_mapping().then(|| FileMapping::new(inode));
            if let Some(file_mapping) = &file_mapping {
                inner.attach_file_mapping(file_mapping.clone());
            }
            VfsInode {
                inner,
                file_mapping,
                lifecycle: SleepRwLockOnStack::new(InodeLifecycleState::default(), "VfsInode::lifecycle"),
                lock_state: SpinLock::new(InodeLockState::new(), "VfsInode::lock_state"),
                seal_state: SpinLock::new(InodeSealState::new(), "VfsInode::seal_state"),
                fifo_pipe: LazyInitedCell::new("Inode::fifo_pipe"),
                #[cfg(feature = "fanotify")]
                fanotify: LazyInitedCell::new("Inode::fanotify"),
            }
        })
    }

    pub fn inner(&self) -> &T {
        &self.inner
    }

    fn current_seals(&self) -> Option<FileSealFlags> {
        self.seal_state.lock().seals
    }

    fn check_write_seals(&self, offset: usize, len: usize) -> SysResult<()> {
        if len == 0 {
            return Ok(());
        }

        let Some(seals) = self.current_seals() else {
            return Ok(());
        };

        if seals.intersects(FileSealFlags::F_SEAL_WRITE | FileSealFlags::F_SEAL_FUTURE_WRITE) {
            return Err(Errno::EPERM);
        }

        let end = offset.checked_add(len).ok_or(Errno::EFBIG)?;
        let file_size = usize::try_from(self.inner.size()?).map_err(|_| Errno::EFBIG)?;
        if end > file_size && seals.contains(FileSealFlags::F_SEAL_GROW) {
            return Err(Errno::EPERM);
        }

        Ok(())
    }

    fn check_chmod_seals(&self, mode: Mode) -> SysResult<()> {
        let Some(seals) = self.current_seals() else {
            return Ok(());
        };
        if !seals.contains(FileSealFlags::F_SEAL_EXEC) {
            return Ok(());
        }

        let exec_bits = Mode::S_IXUSR | Mode::S_IXGRP | Mode::S_IXOTH;
        if (self.inner.mode()? & exec_bits) != (mode & exec_bits) {
            return Err(Errno::EPERM);
        }

        Ok(())
    }

    fn check_truncate_seals(&self, new_size: u64) -> SysResult<()> {
        let Some(seals) = self.current_seals() else {
            return Ok(());
        };

        let old_size = self.inner.size()?;
        if new_size < old_size && seals.contains(FileSealFlags::F_SEAL_SHRINK) {
            return Err(Errno::EPERM);
        }
        if new_size > old_size && seals.contains(FileSealFlags::F_SEAL_GROW) {
            return Err(Errno::EPERM);
        }

        Ok(())
    }

    fn check_fallocate_seals(&self, flags: FileFallocateFlags, offset: u64, len: u64) -> SysResult<()> {
        let Some(seals) = self.current_seals() else {
            return Ok(());
        };

        if flags.is_empty() {
            let new_size = offset.checked_add(len).ok_or(Errno::EFBIG)?;
            if new_size > self.inner.size()? && seals.contains(FileSealFlags::F_SEAL_GROW) {
                return Err(Errno::EPERM);
            }
            return Ok(());
        }

        if flags == (FileFallocateFlags::FALLOC_FL_KEEP_SIZE | FileFallocateFlags::FALLOC_FL_PUNCH_HOLE)
            && seals.intersects(FileSealFlags::F_SEAL_WRITE | FileSealFlags::F_SEAL_FUTURE_WRITE)
        {
            return Err(Errno::EPERM);
        }

        Ok(())
    }
}

impl<T: InodeOps> Deref for VfsInode<T> {
    type Target = T;

    fn deref(&self) -> &Self::Target {
        self.inner()
    }
}

pub fn downcast_inode<T: InodeOps>(inode: &Arc<Inode>) -> Option<&T> {
    inode.downcast_ref::<VfsInode<T>>().map(VfsInode::inner)
}

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

struct InodeSealState {
    seals: Option<FileSealFlags>,
    shared_mmap_count: usize,
    writable_shared_mmap_count: usize,
}

impl InodeSealState {
    fn new() -> Self {
        Self {
            seals: None,
            shared_mmap_count: 0,
            writable_shared_mmap_count: 0,
        }
    }
}

pub fn release_bsd_flock(inode: &Arc<Inode>, owner: usize) {
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

impl<T: InodeOps> InodeSealOps for VfsInode<T> {
    fn init_seals(&self, seals: FileSealFlags) -> SysResult<()> {
        if self.inner.inode_type()? != FileType::Regular {
            return Err(Errno::EINVAL);
        }

        let mut state = self.seal_state.lock();
        if state.seals.is_some() {
            return Err(Errno::EINVAL);
        }
        state.seals = Some(seals);
        Ok(())
    }

    fn seals(&self) -> SysResult<FileSealFlags> {
        self.seal_state.lock().seals.ok_or(Errno::EINVAL)
    }

    fn add_seals(&self, seals: FileSealFlags) -> SysResult<()> {
        let mut state = self.seal_state.lock();
        let current = state.seals.ok_or(Errno::EINVAL)?;
        if current.contains(FileSealFlags::F_SEAL_SEAL) {
            return Err(Errno::EPERM);
        }
        if seals.contains(FileSealFlags::F_SEAL_WRITE) && state.writable_shared_mmap_count > 0 {
            return Err(Errno::EBUSY);
        }

        state.seals = Some(current | seals);
        Ok(())
    }

    fn begin_shared_mmap(&self, writable: bool) {
        let mut state = self.seal_state.lock();
        if state.seals.is_none() {
            return;
        }

        state.shared_mmap_count = state
            .shared_mmap_count
            .checked_add(1)
            .expect("InodeSealState::shared_mmap_count overflow");
        if writable {
            state.writable_shared_mmap_count = state
                .writable_shared_mmap_count
                .checked_add(1)
                .expect("InodeSealState::writable_shared_mmap_count overflow");
        }
    }

    fn update_shared_mmap_writable(&self, old_writable: bool, new_writable: bool) {
        if old_writable == new_writable {
            return;
        }

        let mut state = self.seal_state.lock();
        if state.seals.is_none() {
            return;
        }

        if new_writable {
            state.writable_shared_mmap_count = state
                .writable_shared_mmap_count
                .checked_add(1)
                .expect("InodeSealState::writable_shared_mmap_count overflow");
        } else {
            debug_assert!(
                state.writable_shared_mmap_count > 0,
                "InodeSealState::writable_shared_mmap_count underflow"
            );
            state.writable_shared_mmap_count = state.writable_shared_mmap_count.saturating_sub(1);
        }
    }

    fn end_shared_mmap(&self, writable: bool) {
        let mut state = self.seal_state.lock();
        if state.seals.is_none() {
            return;
        }

        debug_assert!(
            state.shared_mmap_count > 0,
            "InodeSealState::shared_mmap_count underflow"
        );
        state.shared_mmap_count = state.shared_mmap_count.saturating_sub(1);

        if writable {
            debug_assert!(
                state.writable_shared_mmap_count > 0,
                "InodeSealState::writable_shared_mmap_count underflow"
            );
            state.writable_shared_mmap_count = state.writable_shared_mmap_count.saturating_sub(1);
        }
    }
}

pub trait VfsInodeOps: DowncastSync {
    fn get_ino(&self) -> u32;

    fn type_name(&self) -> &'static str;

    fn block_driver(&self) -> SysResult<Option<Arc<dyn BlockDriverOps>>>;

    fn char_driver(&self) -> SysResult<Option<Arc<dyn CharDriverOps>>>;

    fn lock_state(&self) -> Option<&SpinLock<InodeLockState>>;

    fn as_seal_ops(&self) -> Option<&dyn InodeSealOps>;

    fn lifecycle(&self) -> &SleepRwLockOnStack<InodeLifecycleState>;

    fn begin_write_open(&self) -> SysResult<()>;

    fn end_write_open(&self);

    fn begin_exec(&self) -> SysResult<()>;

    fn increment_exec_count(&self);

    fn end_exec(&self);

    fn create(&self, name: &str, mode: Mode, owner: Owner) -> SysResult<Arc<Inode>>;

    fn mknod(&self, name: &str, mode: Mode, owner: Owner, dev: u64) -> SysResult<Arc<Inode>>;

    fn link(&self, name: &str, target: &Arc<Inode>) -> SysResult<()>;

    fn unlink(&self, name: &str) -> SysResult<()>;

    fn rmdir(&self, name: &str) -> SysResult<()>;

    fn symlink(&self, target: &str) -> SysResult<()>;

    fn readat(&self, buf: &mut [u8], offset: usize, direct: bool) -> SysResult<usize>;

    fn writeat(&self, buf: &[u8], offset: usize) -> SysResult<usize>;

    fn read_to_user(&self, ubuf: &UAddrSpaceBuffer, offset: usize, direct: bool) -> SysResult<usize>;

    fn write_from_user(&self, ubuf: &UAddrSpaceBuffer, offset: usize, direct: bool) -> SysResult<usize>;

    fn get_dent(&self, index: usize) -> SysResult<Option<(DirResult, usize)>>;

    fn lookup(&self, name: &str) -> SysResult<u32>;

    fn rename(&self, old_name: &str, source: &Arc<Inode>, new_parent: &Arc<Inode>, new_name: &str) -> SysResult<()>;

    fn readlink(&self, buf: &mut [u8]) -> SysResult<Option<usize>>;

    fn follow_magic_link(&self) -> SysResult<Option<Arc<Dentry>>>;

    fn size(&self) -> SysResult<u64>;

    fn load_raw_page(&self, file_page_index: usize) -> SysResult<Option<PhysPageFrame>>;

    fn acquire_mmap_shared_page(&self, file_page_index: usize) -> SysResult<Option<SharedFilePage>>;

    fn file_mapping(&self) -> Option<&FileMapping>;

    fn writeback_mmap_shared_page(&self, file_page_index: usize, frame: &PhysPageFrame) -> SysResult<()>;

    fn mode(&self) -> SysResult<Mode>;

    fn check_perm(&self, perm: &Perm) -> SysResult<bool>;

    fn chmod(&self, mode: Mode) -> SysResult<()>;

    fn owner(&self) -> SysResult<(Uid, Uid)>;

    fn chown(&self, uid: Option<Uid>, gid: Option<Uid>) -> SysResult<()>;

    fn inode_type(&self) -> SysResult<FileType>;

    fn sync(&self) -> SysResult<()>;

    fn ioctl(&self, request: usize, arg: usize, addrspace: &AddrSpace) -> SysResult<usize>;

    fn fstat(&self) -> SysResult<FileStat>;

    fn truncate(&self, new_size: u64) -> SysResult<()>;

    fn fallocate(&self, flags: FileFallocateFlags, offset: u64, len: u64) -> SysResult<()>;

    fn update_atime(&self, time: &Duration) -> SysResult<()>;

    fn update_mtime(&self, time: &Duration) -> SysResult<()>;

    fn update_ctime(&self, time: &Duration) -> SysResult<()>;

    fn update_mtime_ctime(&self, time: &Duration) -> SysResult<()>;

    fn open_file(self: Arc<Self>, dentry: Option<Arc<Dentry>>, flags: FileFlags) -> SysResult<Arc<dyn FileOps>>;

    fn wrap_file(self: Arc<Self>, dentry: Option<Arc<Dentry>>, flags: FileFlags) -> Arc<dyn FileOps>;

    fn fifo_pipe_inner(&self) -> Arc<PipeInner>;

    #[cfg(feature = "fanotify")]
    fn fanotify(&self) -> Option<Arc<Fanotify>>;

    #[cfg(feature = "fanotify")]
    fn ensure_fanotify(&self) -> Arc<Fanotify>;
}

impl<T: InodeOps> VfsInodeOps for VfsInode<T> {
    fn get_ino(&self) -> u32 {
        self.inner.get_ino()
    }

    fn type_name(&self) -> &'static str {
        self.inner.type_name()
    }

    fn block_driver(&self) -> SysResult<Option<Arc<dyn BlockDriverOps>>> {
        self.inner.block_driver()
    }

    fn char_driver(&self) -> SysResult<Option<Arc<dyn CharDriverOps>>> {
        self.inner.char_driver()
    }

    fn lock_state(&self) -> Option<&SpinLock<InodeLockState>> {
        Some(&self.lock_state)
    }

    fn as_seal_ops(&self) -> Option<&dyn InodeSealOps> {
        Some(self)
    }

    fn lifecycle(&self) -> &SleepRwLockOnStack<InodeLifecycleState> {
        &self.lifecycle
    }

    fn begin_write_open(&self) -> SysResult<()> {
        self.inner.begin_write_open()?;

        let mut lock_state = self.lock_state.lock();
        if lock_state.exec_count() > 0 {
            return Err(Errno::ETXTBSY);
        }
        lock_state.increment_writer_count();
        Ok(())
    }

    fn end_write_open(&self) {
        self.lock_state.lock().decrement_writer_count();
        self.inner.end_write_open();
    }

    fn begin_exec(&self) -> SysResult<()> {
        self.inner.begin_exec()?;

        let mut lock_state = self.lock_state.lock();
        if lock_state.writer_count() > 0 {
            return Err(Errno::ETXTBSY);
        }
        lock_state.increment_exec_count();
        Ok(())
    }

    fn increment_exec_count(&self) {
        self.lock_state.lock().increment_exec_count();
        self.inner.increment_exec_count();
    }

    fn end_exec(&self) {
        self.lock_state.lock().decrement_exec_count();
        self.inner.end_exec();
    }

    fn create(&self, name: &str, mode: Mode, owner: Owner) -> SysResult<Arc<Inode>> {
        Ok(VfsInode::new(self.inner.create(name, mode, owner)?))
    }

    fn mknod(&self, name: &str, mode: Mode, owner: Owner, dev: u64) -> SysResult<Arc<Inode>> {
        Ok(VfsInode::new(self.inner.mknod(name, mode, owner, dev)?))
    }

    fn link(&self, name: &str, target: &Arc<Inode>) -> SysResult<()> {
        let target = downcast_inode::<T>(target).ok_or(Errno::EXDEV)?;
        self.inner.link(name, target)
    }

    fn unlink(&self, name: &str) -> SysResult<()> {
        self.inner.unlink(name)
    }

    fn rmdir(&self, name: &str) -> SysResult<()> {
        self.inner.rmdir(name)
    }

    fn symlink(&self, target: &str) -> SysResult<()> {
        self.inner.symlink(target)
    }

    fn readat(&self, buf: &mut [u8], offset: usize, direct: bool) -> SysResult<usize> {
        let _lifecycle = self.lifecycle.read();
        self.inner.readat(buf, offset, direct)
    }

    fn writeat(&self, buf: &[u8], offset: usize) -> SysResult<usize> {
        let _lifecycle = self.lifecycle.read();
        self.check_write_seals(offset, buf.len())?;
        self.inner.writeat(buf, offset)
    }

    fn read_to_user(&self, ubuf: &UAddrSpaceBuffer, offset: usize, direct: bool) -> SysResult<usize> {
        let _lifecycle = self.lifecycle.read();
        self.inner.read_to_user(ubuf, offset, direct)
    }

    fn write_from_user(&self, ubuf: &UAddrSpaceBuffer, offset: usize, direct: bool) -> SysResult<usize> {
        let _lifecycle = self.lifecycle.read();
        self.check_write_seals(offset, ubuf.length())?;
        self.inner.write_from_user(ubuf, offset, direct)
    }

    fn get_dent(&self, index: usize) -> SysResult<Option<(DirResult, usize)>> {
        self.inner.get_dent(index)
    }

    fn lookup(&self, name: &str) -> SysResult<u32> {
        self.inner.lookup(name)
    }

    fn rename(&self, old_name: &str, source: &Arc<Inode>, new_parent: &Arc<Inode>, new_name: &str) -> SysResult<()> {
        let source = downcast_inode::<T>(source).ok_or(Errno::EXDEV)?;
        let new_parent = downcast_inode::<T>(new_parent).ok_or(Errno::EXDEV)?;
        self.inner.rename(old_name, source, new_parent, new_name)
    }

    fn readlink(&self, buf: &mut [u8]) -> SysResult<Option<usize>> {
        self.inner.readlink(buf)
    }

    fn follow_magic_link(&self) -> SysResult<Option<Arc<Dentry>>> {
        self.inner.follow_magic_link()
    }

    fn size(&self) -> SysResult<u64> {
        self.inner.size()
    }

    fn load_raw_page(&self, file_page_index: usize) -> SysResult<Option<PhysPageFrame>> {
        self.inner.load_raw_page(file_page_index)
    }

    fn acquire_mmap_shared_page(&self, file_page_index: usize) -> SysResult<Option<SharedFilePage>> {
        let Some(file_mapping) = &self.file_mapping else {
            return self
                .inner
                .mmap_shared_page(file_page_index)
                .map(|page| page.map(SharedFilePage::Stable));
        };
        file_mapping
            .acquire_mapped_page(file_page_index, || self.inner.load_raw_page(file_page_index))
            .map(|page| page.map(SharedFilePage::Swappable))
    }

    fn file_mapping(&self) -> Option<&FileMapping> {
        self.file_mapping.as_deref()
    }

    fn writeback_mmap_shared_page(&self, file_page_index: usize, frame: &PhysPageFrame) -> SysResult<()> {
        self.inner.writeback_mmap_shared_page(file_page_index, frame)
    }

    fn mode(&self) -> SysResult<Mode> {
        self.inner.mode()
    }

    fn check_perm(&self, perm: &Perm) -> SysResult<bool> {
        self.inner.check_perm(perm)
    }

    fn chmod(&self, mode: Mode) -> SysResult<()> {
        self.check_chmod_seals(mode)?;
        self.inner.chmod(mode)
    }

    fn owner(&self) -> SysResult<(Uid, Uid)> {
        self.inner.owner()
    }

    fn chown(&self, uid: Option<Uid>, gid: Option<Uid>) -> SysResult<()> {
        self.inner.chown(uid, gid)
    }

    fn inode_type(&self) -> SysResult<FileType> {
        self.inner.inode_type()
    }

    fn sync(&self) -> SysResult<()> {
        let _lifecycle = self.lifecycle.read();
        self.inner.sync()
    }

    fn ioctl(&self, request: usize, arg: usize, addrspace: &AddrSpace) -> SysResult<usize> {
        self.inner.ioctl(request, arg, addrspace)
    }

    fn fstat(&self) -> SysResult<FileStat> {
        self.inner.fstat()
    }

    fn truncate(&self, new_size: u64) -> SysResult<()> {
        let _lifecycle = self.lifecycle.write();
        self.check_truncate_seals(new_size)?;
        self.inner.truncate(new_size)
    }

    fn fallocate(&self, flags: FileFallocateFlags, offset: u64, len: u64) -> SysResult<()> {
        let _lifecycle = self.lifecycle.write();
        self.check_fallocate_seals(flags, offset, len)?;
        self.inner.fallocate(flags, offset, len)
    }

    fn update_atime(&self, time: &Duration) -> SysResult<()> {
        self.inner.update_atime(time)
    }

    fn update_mtime(&self, time: &Duration) -> SysResult<()> {
        self.inner.update_mtime(time)
    }

    fn update_ctime(&self, time: &Duration) -> SysResult<()> {
        self.inner.update_ctime(time)
    }

    fn update_mtime_ctime(&self, time: &Duration) -> SysResult<()> {
        self.inner.update_mtime_ctime(time)
    }

    fn open_file(self: Arc<Self>, dentry: Option<Arc<Dentry>>, flags: FileFlags) -> SysResult<Arc<dyn FileOps>> {
        let inode: Arc<Inode> = self.clone();
        self.inner.open_file(inode, dentry, flags)
    }

    fn wrap_file(self: Arc<Self>, dentry: Option<Arc<Dentry>>, flags: FileFlags) -> Arc<dyn FileOps> {
        let inode: Arc<Inode> = self.clone();
        self.inner.wrap_file(inode, dentry, flags)
    }

    fn fifo_pipe_inner(&self) -> Arc<PipeInner> {
        self.fifo_pipe
            .get_or_init(|| Arc::new(PipeInner::new(config::PIPE_CAPACITY)))
    }

    #[cfg(feature = "fanotify")]
    fn fanotify(&self) -> Option<Arc<Fanotify>> {
        self.fanotify.get()
    }

    #[cfg(feature = "fanotify")]
    fn ensure_fanotify(&self) -> Arc<Fanotify> {
        self.fanotify.get_or_init(|| Arc::new(Fanotify::new()))
    }
}

impl_downcast!(sync VfsInodeOps);

pub trait InodeOps: Send + Sync + Sized + 'static {
    fn attach_file_mapping(&mut self, _mapping: Arc<FileMapping>) {}

    fn get_ino(&self) -> u32;

    fn type_name(&self) -> &'static str;

    fn block_driver(&self) -> SysResult<Option<Arc<dyn BlockDriverOps>>> {
        Ok(None)
    }

    fn char_driver(&self) -> SysResult<Option<Arc<dyn CharDriverOps>>> {
        Ok(None)
    }

    fn begin_write_open(&self) -> SysResult<()> {
        Ok(())
    }

    fn end_write_open(&self) {}

    fn begin_exec(&self) -> SysResult<()> {
        Ok(())
    }

    fn increment_exec_count(&self) {}

    fn end_exec(&self) {}

    fn create(&self, _name: &str, _mode: Mode, _owner: Owner) -> SysResult<Self> {
        Err(Errno::EOPNOTSUPP)
    }

    fn mknod(&self, name: &str, mode: Mode, owner: Owner, _dev: u64) -> SysResult<Self> {
        if (mode & Mode::S_IFMT) == Mode::S_IFIFO {
            self.create(name, mode, owner)
        } else {
            Err(Errno::EOPNOTSUPP)
        }
    }

    fn link(&self, name: &str, target: &Self) -> SysResult<()> {
        let _ = name;
        let _ = target;
        Err(Errno::EOPNOTSUPP)
    }

    fn unlink(&self, _name: &str) -> SysResult<()> {
        Err(Errno::EOPNOTSUPP)
    }

    fn rmdir(&self, name: &str) -> SysResult<()> {
        self.unlink(name)
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
            let mut kbuf = kbuf?;
            let n = self.readat(&mut kbuf, current_offset, direct)?;
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
            let n = self.writeat(&kbuf, current_offset)?;
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

    fn rename(&self, _old_name: &str, _source: &Self, _new_parent: &Self, _new_name: &str) -> SysResult<()> {
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

    fn load_raw_page(&self, file_page_index: usize) -> SysResult<Option<PhysPageFrame>> {
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
        Ok(Some(frame))
    }

    fn supports_file_mapping(&self) -> bool {
        false
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

    fn update_mtime_ctime(&self, time: &Duration) -> SysResult<()> {
        self.update_mtime(time)?;
        self.update_ctime(time)
    }

    fn open_file(
        &self,
        inode: Arc<Inode>,
        dentry: Option<Arc<Dentry>>,
        flags: FileFlags,
    ) -> SysResult<Arc<dyn FileOps>> {
        Ok(self.wrap_file(inode, dentry, flags))
    }

    fn wrap_file(&self, inode: Arc<Inode>, dentry: Option<Arc<Dentry>>, flags: FileFlags) -> Arc<dyn FileOps>;
}

impl<T: InodeOps> InodeOps for Arc<T> {
    fn get_ino(&self) -> u32 {
        self.as_ref().get_ino()
    }

    fn type_name(&self) -> &'static str {
        self.as_ref().type_name()
    }

    fn begin_write_open(&self) -> SysResult<()> {
        self.as_ref().begin_write_open()
    }

    fn end_write_open(&self) {
        self.as_ref().end_write_open()
    }

    fn begin_exec(&self) -> SysResult<()> {
        self.as_ref().begin_exec()
    }

    fn increment_exec_count(&self) {
        self.as_ref().increment_exec_count()
    }

    fn end_exec(&self) {
        self.as_ref().end_exec()
    }

    fn create(&self, name: &str, mode: Mode, owner: Owner) -> SysResult<Self> {
        Ok(Arc::new(self.as_ref().create(name, mode, owner)?))
    }

    fn mknod(&self, name: &str, mode: Mode, owner: Owner, dev: u64) -> SysResult<Self> {
        Ok(Arc::new(self.as_ref().mknod(name, mode, owner, dev)?))
    }

    fn link(&self, name: &str, target: &Self) -> SysResult<()> {
        self.as_ref().link(name, target.as_ref())
    }

    fn unlink(&self, name: &str) -> SysResult<()> {
        self.as_ref().unlink(name)
    }

    fn rmdir(&self, name: &str) -> SysResult<()> {
        self.as_ref().rmdir(name)
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

    fn rename(&self, old_name: &str, source: &Self, new_parent: &Self, new_name: &str) -> SysResult<()> {
        self.as_ref()
            .rename(old_name, source.as_ref(), new_parent.as_ref(), new_name)
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

    fn supports_file_mapping(&self) -> bool {
        self.as_ref().supports_file_mapping()
    }

    fn writeback_mmap_shared_page(&self, file_page_index: usize, frame: &PhysPageFrame) -> SysResult<()> {
        self.as_ref().writeback_mmap_shared_page(file_page_index, frame)
    }

    fn mode(&self) -> SysResult<Mode> {
        self.as_ref().mode()
    }

    fn check_perm(&self, perm: &Perm) -> SysResult<bool> {
        self.as_ref().check_perm(perm)
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
        inode: Arc<Inode>,
        dentry: Option<Arc<Dentry>>,
        flags: FileFlags,
    ) -> SysResult<Arc<dyn FileOps>> {
        self.as_ref().open_file(inode, dentry, flags)
    }

    fn wrap_file(&self, inode: Arc<Inode>, dentry: Option<Arc<Dentry>>, flags: FileFlags) -> Arc<dyn FileOps> {
        self.as_ref().wrap_file(inode, dentry, flags)
    }
}
