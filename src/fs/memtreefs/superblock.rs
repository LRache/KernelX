use alloc::sync::Arc;
use alloc::collections::BTreeMap;

use crate::kernel::errno::{SysResult, Errno};
use crate::fs::filesystem::SuperBlockOps;
use crate::fs::{InodeOps, Mode};
use crate::klib::SpinLock;

use super::inode::{InodeMeta, Inode as MemInode};

pub trait StaticFsInfo: Send + Sync + 'static {
    fn type_name() -> &'static str; 
}

pub struct SuperBlockInner {
    inodes: BTreeMap<u32, Arc<dyn InodeOps>>,
    max_inode: u32,
}

impl SuperBlockInner {
    pub fn new() -> Self {
        Self {
            inodes: BTreeMap::new(),
            max_inode: 0,
        }
    }

    pub fn alloc_inode<F>(&mut self, f: F) -> (u32, Arc<dyn InodeOps>)
    where F: FnOnce(u32) -> Arc<dyn InodeOps> {
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

    pub fn insert_inode(&mut self, ino: u32, inode: Arc<dyn InodeOps>) {
        self.inodes.insert(ino, inode);
    }

    pub fn remove_inode(&mut self, ino: u32) {
        self.inodes.remove(&ino);
    }

    pub fn get_inode(&self, ino: u32) -> SysResult<Arc<dyn InodeOps>> {
        let inode = self.inodes.get(&ino).ok_or(Errno::ENOENT)?.clone();
        Ok(inode)
    }
}
pub struct SuperBlock<T: StaticFsInfo> {
    inner: Arc<SpinLock<SuperBlockInner>>,
    _marker: core::marker::PhantomData<T>,
}

impl<T: StaticFsInfo> SuperBlock<T> {
    pub fn new() -> Self {
        let inner = Arc::new(SpinLock::new(SuperBlockInner::new()));

        {
            inner.lock().alloc_inode(|ino| {
                Arc::new(MemInode::<T>::new(
                    ino,
                    InodeMeta::new(
                        Mode::from_bits(Mode::S_IFDIR.bits() | 0o755).unwrap(),
                        ino, 0
                    ),
                    inner.clone()
                ))
            });
        }

        Self { 
            inner, 
            _marker: core::marker::PhantomData 
        }
    }

    pub fn root_inode(&self) -> Arc<MemInode<T>> {
        let root = self.inner.lock().get_inode(0).unwrap();
        if let Ok(root) = root.downcast_arc::<MemInode<T>>() {
            root
        } else {
            unreachable!()
        }
    }

    pub fn alloc_inode_number(&self) -> u32 {
        self.inner.lock().alloc_inode_number()
    }
}

impl<T: StaticFsInfo> SuperBlockOps for SuperBlock<T> {
    fn get_root_ino(&self) -> u32 {
        0
    }

    fn get_inode(&self, ino: u32) -> SysResult<Arc<dyn InodeOps>> {
        let inode = self.inner.lock().get_inode(ino)?;
        Ok(inode)
    }

    fn create_temp(&self, mode: Mode) -> SysResult<Arc<dyn InodeOps>> {
        let mut inner = self.inner.lock();
        let ino = inner.alloc_inode_number();

        let inode: Arc<dyn InodeOps> = Arc::new(MemInode::<T>::new(
            ino,
            InodeMeta::new(mode, ino, self.get_root_ino()), self.inner.clone()
        ));
        inner.insert_inode(ino, inode.clone());

        Ok(inode)
    }
}


