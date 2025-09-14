use core::cell::UnsafeCell;
use core::mem::MaybeUninit;
use alloc::boxed::Box;
use alloc::sync::Arc;
use alloc::string::String;
use alloc::collections::BTreeMap;
use spin::mutex::Mutex;

use crate::kernel::errno::{Errno, SysResult};
use crate::fs::inode::LockedInode;
use crate::fs::filesystem::FileSystem;
use crate::fs::inode;

use super::rootfs::RootFileSystem;
use super::dentry::Dentry;
use super::SuperBlockTable;

pub struct VirtualFileSystem {
    cache: inode::Cache,
    pub superblock_table: Mutex<SuperBlockTable>,
    pub fstype_map: Mutex<BTreeMap<String, Box<dyn FileSystem>>>,
    root: UnsafeCell<MaybeUninit<Arc<Dentry>>>,
}

impl VirtualFileSystem {
    pub const fn new() -> Self {
        let vfs = VirtualFileSystem {
            cache: inode::Cache::new(),
            superblock_table: Mutex::new(SuperBlockTable::new()),
            fstype_map: Mutex::new(BTreeMap::new()),
            root: UnsafeCell::new(MaybeUninit::zeroed())
        };

        vfs
    }

    pub fn init(&self) {
        assert!(self.superblock_table.lock().is_empty());

        self.superblock_table.lock()
                             .alloc(&RootFileSystem::new(), None).unwrap();

        unsafe {
           self.root.get().write(
                MaybeUninit::new(Arc::new(Dentry::root(&self.open_inode(&inode::Index::root()).unwrap())))
            );
        }
        
    }

    pub fn get_root(&self) -> &Arc<Dentry> {
        unsafe { self.root.get().as_ref().unwrap().assume_init_ref() }
    }

    pub fn lookup_dentry(&self, dir: &Arc<Dentry>, path: &str) -> SysResult<Arc<Dentry>> {
        let mut current = dir.clone();
        if path == "/" {
            return Ok(current);
        }

        for part in path.split('/').filter(|s| !s.is_empty()) {
            let next = current.lookup(part)?;
            current = next;
        }

        Ok(current)
    }

    pub fn open_inode(&self, index: &inode::Index) -> SysResult<Arc<LockedInode>> {
        if let Some(inode) = self.cache.find(index) {
            Ok(inode)
        } else {
            let superblock_table = self.superblock_table.lock();
            let superblock = superblock_table.get(index.sno).ok_or(Errno::ENOENT)?;
            let inode = superblock.get_inode(index.ino)?;

            let inode = Arc::new(LockedInode::new(index, inode));
            self.cache.insert(index, inode.clone())?;
            Ok(inode)
        }
    }
}

unsafe impl Sync for VirtualFileSystem {}

static VFS: VirtualFileSystem = VirtualFileSystem::new();

pub fn init() {
    VFS.init();
}

pub fn vfs() -> &'static VirtualFileSystem {
    &VFS
}
 