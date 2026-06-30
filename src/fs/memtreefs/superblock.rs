use alloc::collections::BTreeMap;
use alloc::sync::Arc;

use crate::arch;
use crate::fs::filesystem::VfsSuperBlockOps;
use crate::fs::{Inode, Mode, VfsInode};
use crate::kernel::errno::{Errno, SysResult};
#[cfg(feature = "fanotify")]
use crate::kernel::event::Fanotify;
use crate::kernel::scheduler::current;
use crate::kernel::uapi::Statfs;
#[cfg(feature = "fanotify")]
use crate::klib::LazyInitedCell;
use crate::klib::SpinLock;

use super::inode::{InodeMeta, MemInodeOps, RegularInode};

pub trait StaticFsInfo: Send + Sync + 'static {
    const MAX_FILENAME_LEN: Option<usize> = None;

    fn statfs_magic() -> u64;
    fn type_name() -> &'static str;

    fn check_filename(name: &str) -> SysResult<()> {
        if Self::MAX_FILENAME_LEN.is_some_and(|max_len| name.len() > max_len) {
            return Err(Errno::ENAMETOOLONG);
        }
        Ok(())
    }
}

pub struct SuperBlockInner<T: StaticFsInfo> {
    inodes: BTreeMap<u32, Arc<dyn MemInodeOps<T>>>,
    max_inode: u32,
}

impl<T: StaticFsInfo> SuperBlockInner<T> {
    pub fn new() -> Self {
        Self {
            inodes: BTreeMap::new(),
            max_inode: 0,
        }
    }

    pub fn alloc_inode<F>(&mut self, f: F) -> (u32, Arc<dyn MemInodeOps<T>>)
    where
        F: FnOnce(u32) -> Arc<dyn MemInodeOps<T>>,
    {
        let ino = self.max_inode;
        self.max_inode += 1;
        let inode = f(ino);
        self.inodes.insert(ino, inode.clone());
        (ino, inode)
    }

    pub fn alloc_inode_number(&mut self) -> u32 {
        let ino = self.max_inode;
        self.max_inode += 1;
        ino
    }

    pub fn insert_inode(&mut self, ino: u32, inode: Arc<dyn MemInodeOps<T>>) {
        self.inodes.insert(ino, inode);
    }

    pub fn remove_inode(&mut self, ino: u32) {
        self.inodes.remove(&ino);
    }

    pub fn get_inode(&self, ino: u32) -> SysResult<Arc<dyn MemInodeOps<T>>> {
        let inode = self.inodes.get(&ino).ok_or(Errno::ENOENT)?.clone();
        Ok(inode)
    }
}
pub struct SuperBlock<T: StaticFsInfo> {
    inner: Arc<SpinLock<SuperBlockInner<T>>>,
    #[cfg(feature = "fanotify")]
    fanotify: LazyInitedCell<Arc<Fanotify>>,
    read_only: bool,
    _marker: core::marker::PhantomData<T>,
}

impl<T: StaticFsInfo> SuperBlock<T> {
    pub fn new(read_only: bool) -> Self {
        let inner = Arc::new(SpinLock::new(SuperBlockInner::new(), "SuperBlock::inner"));

        {
            inner.lock().alloc_inode(|ino| {
                Arc::new(RegularInode::<T>::new(
                    ino,
                    InodeMeta::new(Mode::from_bits(Mode::S_IFDIR.bits() | 0o755).unwrap(), ino, 0),
                    inner.clone(),
                ))
            });
        }

        Self {
            inner,
            #[cfg(feature = "fanotify")]
            fanotify: LazyInitedCell::new("SuperBlock::fanotify"),
            read_only,
            _marker: core::marker::PhantomData,
        }
    }

    pub fn root_inode(&self) -> Arc<dyn MemInodeOps<T>> {
        self.inner.lock().get_inode(0).unwrap()
    }

    pub fn alloc_inode_number(&self) -> u32 {
        self.inner.lock().alloc_inode_number()
    }
}

impl<T: StaticFsInfo> VfsSuperBlockOps for SuperBlock<T> {
    fn get_root_ino(&self) -> u32 {
        0
    }

    fn get_inode(&self, ino: u32) -> SysResult<Arc<Inode>> {
        let inode = self.inner.lock().get_inode(ino)?;
        Ok(VfsInode::new(inode))
    }

    #[cfg(feature = "fanotify")]
    fn fanotify(&self) -> Option<Arc<Fanotify>> {
        self.fanotify.get()
    }

    #[cfg(feature = "fanotify")]
    fn ensure_fanotify(&self) -> Option<Arc<Fanotify>> {
        Some(self.fanotify.get_or_init(|| Arc::new(Fanotify::new())))
    }

    fn create_temp(&self, mode: Mode) -> SysResult<Arc<Inode>> {
        let mut inner = self.inner.lock();
        let ino = inner.alloc_inode_number();
        let mut meta = InodeMeta::new(mode, ino, self.get_root_ino());
        meta.owner = (current::fsuid(), current::fsgid());

        let inode: Arc<dyn MemInodeOps<T>> = Arc::new(RegularInode::<T>::new(ino, meta, self.inner.clone()));
        inner.insert_inode(ino, inode.clone());

        Ok(VfsInode::new(inode))
    }

    fn statfs(&self) -> SysResult<Statfs> {
        let mut statfs = Statfs::default();
        statfs.f_type = T::statfs_magic();
        statfs.f_bsize = arch::PGSIZE as u64;
        statfs.f_blocks = 0;
        statfs.f_bfree = 0;
        statfs.f_bavail = 0;
        statfs.f_flag = self.statfs_flags().bits();
        Ok(statfs)
    }

    fn is_readonly(&self) -> bool {
        self.read_only
    }

    fn type_name(&self) -> &'static str {
        T::type_name()
    }
}
