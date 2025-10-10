use core::cell::UnsafeCell;
use core::mem::MaybeUninit;
use alloc::boxed::Box;
use alloc::sync::Arc;
use alloc::string::String;
use alloc::collections::BTreeMap;
use alloc::vec::Vec;
use spin::mutex::Mutex;

use crate::kernel::errno::{Errno, SysResult};
use crate::fs::inode::Inode;
use crate::fs::filesystem::FileSystem;
use crate::fs::inode;
use crate::driver::block::BlockDriver;
use crate::kdebug;

use super::rootfs::RootFileSystem;
use super::dentry::Dentry;
use super::SuperBlockTable;

pub struct VirtualFileSystem {
    cache: inode::Cache,
    mountpoint: Mutex<Vec<Arc<Dentry>>>,
    pub superblock_table: Mutex<SuperBlockTable>,
    pub fstype_map: Mutex<BTreeMap<String, Box<dyn FileSystem>>>,
    root: UnsafeCell<MaybeUninit<Arc<Dentry>>>,
}

impl VirtualFileSystem {
    pub const fn new() -> Self {
        let vfs = VirtualFileSystem {
            cache: inode::Cache::new(),
            mountpoint: Mutex::new(Vec::new()),
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
                MaybeUninit::new(Arc::new(Dentry::root(&self.load_inode(0, 0).unwrap())))
            );
        }
        
    }

    pub fn get_root(&self) -> &Arc<Dentry> {
        unsafe { self.root.get().as_ref().unwrap().assume_init_ref() }
    }

    pub fn mount(&self, path: &str, fstype_name: &str, device: Option<Box<dyn BlockDriver>>) -> SysResult<()> {
        let dentry = self.lookup_dentry(self.get_root(), path)?;

        let fstype_map = self.fstype_map.lock();    
        let fstype = fstype_map.get(fstype_name).ok_or(Errno::ENOENT)?;

        let (sno, root_ino) = {
            let mut superblock_table = self.superblock_table.lock();
            let sno = superblock_table.alloc(fstype, device)?;
            (sno, superblock_table.get(sno).unwrap().get_root_ino())
        };

        let root_inode = self.load_inode(sno, root_ino)?;

        dentry.mount(&root_inode);
        
        self.mountpoint.lock().push(dentry);
        
        Ok(())
    }

    pub fn lookup_dentry(&self, dir: &Arc<Dentry>, path: &str) -> SysResult<Arc<Dentry>> {
        let mut current = match path.chars().next() {
            Some('/') => self.get_root().clone(),
            _ => dir.clone()
        };

        // TODO: Link to
        current = current.get_mount_to();
        current = current.get_link_to();

        kdebug!("path={}", path);

        path.split('/').filter(|s| !(s.is_empty() || *s == ".")).try_for_each(|part| {
            let next = current.lookup(part)?;
            kdebug!("part={}, next={}.{}", part, next.sno(), next.ino());
            current = next.get_mount_to();
            kdebug!("part={}, current={}.{}", part, current.sno(), current.ino());

            Ok(())
        })?;

        Ok(current)
    }

    pub fn lookup_parent_dentry(&self, dir: &Arc<Dentry>, path: &str) -> SysResult<(Arc<Dentry>, String)> {
        let mut current = match path.chars().next() {
            Some('/') => self.get_root().clone(),
            _ => dir.clone(),
        };

        let parts: Vec<&str> = path.split('/').filter(|s| !s.is_empty()).collect();

        if parts.is_empty() {
            return current.get_parent()
                   .ok_or(Errno::ENOENT)
                   .map(|p| (p, String::from("/")));
        }

        for part in &parts[0..parts.len()-1] {
            let next = current.lookup(part)?;
            current = next;
        }

        Ok((current, parts[parts.len()-1].into()))
    }

    pub fn load_inode(&self, sno: u32, ino: u32) -> SysResult<Arc<dyn Inode>> {
        let index = inode::Index { sno, ino };
        if let Some(inode) = self.cache.find(&index) {
            Ok(inode)
        } else {
            let superblock_table = self.superblock_table.lock();
            let superblock = superblock_table.get(sno).ok_or(Errno::ENOENT)?;

            let inode: Arc<dyn Inode> = Arc::from(superblock.get_inode(ino)?);
            self.cache.insert(&index, inode.clone())?;
            
            Ok(inode)
        }
    }

    pub fn sync(&self) -> SysResult<()> {
        self.cache.sync()
    }
}

unsafe impl Sync for VirtualFileSystem {}
 