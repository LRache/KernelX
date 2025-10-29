use alloc::boxed::Box;
use alloc::sync::Arc;
use alloc::collections::BTreeMap;

use crate::kernel::errno::{SysResult, Errno};
use crate::fs::filesystem::SuperBlockOps;
use crate::fs::{InodeOps, Mode, inode};
use crate::klib::SpinLock;

use super::inode::{InodeMeta, Inode};

pub struct SuperBlockInner {
    inode_map: BTreeMap<u32, Arc<SpinLock<InodeMeta>>>,
    max_inode: u32
}

impl SuperBlockInner {
    pub fn new() -> Self {
        let mut inode_map = BTreeMap::new();
        inode_map.insert(0, Arc::new(SpinLock::new(InodeMeta::new(Mode::S_IFDIR))));
        Self {
            inode_map,
            max_inode: 1
        }
    }

    pub fn alloc_inode(&mut self, inode_mode: Mode) -> u32 {
        let ino = self.max_inode;
        self.max_inode += 1;
        self.inode_map.insert(ino, Arc::new(SpinLock::new(InodeMeta::new(inode_mode))));

        ino
    }
}

pub struct SuperBlock {
    sno: u32,
    inner: Arc<SpinLock<SuperBlockInner>>,
}

impl SuperBlock {
    pub fn new(sno: u32) -> Self {
        Self {
            sno,
            inner: Arc::new(SpinLock::new(SuperBlockInner::new())),
        }
    }
}

impl SuperBlockOps for SuperBlock {
    fn get_root_ino(&self) -> u32 {
        0
    }

    fn get_inode(&self, ino: u32) -> SysResult<Box<dyn InodeOps>> {
        let meta = self.inner.lock().inode_map.get(&ino)
            .ok_or(Errno::ENOENT)?
            .clone();
        Ok(Box::new(Inode::new(ino, self.sno, meta, self.inner.clone())))
    }
}
