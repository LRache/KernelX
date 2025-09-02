use core::mem::MaybeUninit;

use alloc::boxed::Box;
use alloc::sync::Arc;
use alloc::vec::Vec;
use alloc::string::String;
use alloc::collections::BTreeMap;
use spin::mutex::Mutex;

use crate::fs::inode::LockedInode;
use crate::fs::filesystem::{FileSystem, SuperBlock};
use crate::fs::mount::Manager as MountManager;
use crate::fs::inode;
use crate::kernel::errno::{Errno, SysResult};
use crate::kdebug;

use super::rootfs::RootFileSystem;
use super::dentry::Dentry;

pub struct SuperBlockTable {
    table: Vec<Option<Arc<dyn SuperBlock>>>
}

impl SuperBlockTable {
    pub const fn new() -> Self {
        SuperBlockTable {
            table: Vec::new(),
        }
    }

    pub fn push(&mut self, fs: Arc<dyn SuperBlock>) -> usize {
        self.table.push(Some(fs));
        self.table.len() - 1
    }

    pub fn next_fsno(&self) -> usize {
        self.table.len()
    }

    pub fn get(&self, sno: u32) -> Option<Arc<dyn SuperBlock>> {
        let fs = self.table.get(sno as usize)?;
        fs.as_ref().cloned()
    }

    pub fn is_empty(&self) -> bool {
        self.table.is_empty()
    }
}

pub struct VirtualFileSystem {
    cache: inode::Cache,
    pub mountmanager: Mutex<MountManager>,
    pub superblock_table: Mutex<SuperBlockTable>,
    pub fstype_map: Mutex<BTreeMap<String, Box<dyn FileSystem>>>,
    root: MaybeUninit<Arc<Dentry>>,
}

impl VirtualFileSystem {
    pub const fn new() -> Self {
        VirtualFileSystem {
            cache: inode::Cache::new(),
            mountmanager: Mutex::new(MountManager::new()),
            superblock_table: Mutex::new(SuperBlockTable::new()),
            fstype_map: Mutex::new(BTreeMap::new()),
            root: MaybeUninit::zeroed()
        }
    }

    pub fn init(&mut self) {
        {
            let fstype_map = self.fstype_map.lock();
            if !fstype_map.is_empty() {
                panic!("VFS already initialized with filesystems");
            }
        }

        let mut superblock_table = self.superblock_table.lock();
        if !superblock_table.is_empty() {
            panic!("VFS already initialized with filesystems");
        }

        superblock_table.push(RootFileSystem::new().create(0, None).unwrap());

        self.root = MaybeUninit::new(
            Arc::new(Dentry::root(&self.open_inode(0, 0).unwrap()))
        );
    }

    fn get_root(&self) -> &Arc<Dentry> {
        unsafe { self.root.assume_init_ref() }
    }

    pub fn lookup_inode(&self, path: &str, start: &Arc<Dentry>) -> Result<inode::Index, Errno> {
        let parts = path.split('/').filter(|s| !s.is_empty());
        
        let mut current;
        if path.starts_with("/") {
            current = self.get_root();
        } else {
            current = start;
        }

        let mounts = VFS.mountmanager.lock();  

        for part in parts {
            if part == "." {
                continue;
            }

            // Check if the current part is a mount point
            if let Some(mount) = mounts.get_mount(current_fsno, current_ino) {
                current_fsno = mount.fsno;
                current_ino = mount.ino;
            }

            kdebug!("part: {}, current_fsno={}, current_ino={}", part, current_fsno, current_ino);

            // Lookup the inode in the current filesystem
            let lookup_result = self.with_inode(current_fsno, current_ino, |inode| {
                inode.lookup(part)
            })?;
            match lookup_result {
                Ok(ino) => current_ino = ino,
                Err(e) => return Err(e),
            };
        }

        Ok(inode::Index { fsno: current_fsno, ino: current_ino })
    }

    pub fn with_inode<F, R>(&self, fsno: usize, ino: usize, f: F) -> Result<R, Errno>
    where
        F: FnOnce(&InodeWrapper) -> R,
    {
        if let Some(inode) = self.inodemanager.get_inode(fsno, ino) {
            Ok(f(&inode))
        } else {
            let superblock_table = self.superblock_table.lock();
            if let Some(filesystem) = superblock_table.get(fsno) {
                return filesystem.get_inode(ino).and_then(|inode| {
                    Ok(f(&InodeWrapper::new_raw(inode)))
                });
            }
            Err(Errno::ENOENT)
        }
    }

    // pub fn open_inode(&self, fsno: usize, ino: usize) -> Result<Arc<InodeWrapper>, Errno> {
    //     if let Some(inode) = self.inodemanager.get_inode(fsno, ino) {
    //         Ok(inode)
    //     } else {
    //         let fs_table = self.superblock_table.lock();
    //         let filesystem = fs_table.get(fsno).ok_or(Errno::ENOENT)?;
    //         let inode = filesystem.get_inode(ino)?;
    //         Ok(self.inodemanager.add_inode(fsno, ino, inode))
    //     }
    // }
    pub fn open_inode(&self, sno: u32, ino: u32) -> SysResult<Arc<LockedInode>> {
        if let Some(inode) = self.cache.find(inode::Index { sno, ino }) {
            Ok(inode)
        } else {
            let superblock_table = self.superblock_table.lock();
            let superblock = superblock_table.get(sno).ok_or(Errno::ENOENT)?;
            let inode = superblock.get_inode(ino)?;
            Ok(self.inodemanager.add_inode(sno, ino, Mutex::new(inode)))
        }
    }
}

pub static mut VFS: VirtualFileSystem = VirtualFileSystem::new();
