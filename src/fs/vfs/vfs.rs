use alloc::collections::BTreeMap;
use alloc::sync::Arc;
use alloc::vec::Vec;
use spin::mutex::Mutex;

use crate::fs::filesystem::FileSystemOps;
use crate::fs::inode;
use crate::fs::inode::InodeOps;
use crate::kernel::errno::{Errno, SysResult};
use crate::klib::InitedCell;

use super::SuperBlockTable;
use super::dentry::Dentry;

pub struct VirtualFileSystem {
    pub(super) cache: inode::Cache,
    pub(super) mountpoint: Mutex<Vec<Arc<Dentry>>>,
    pub superblock_table: Mutex<SuperBlockTable>,
    pub(super) fstype_map: BTreeMap<&'static str, &'static dyn FileSystemOps>,
    pub(super) root: InitedCell<Arc<Dentry>>,
}

impl VirtualFileSystem {
    pub fn new() -> Self {
        let vfs = VirtualFileSystem {
            cache: inode::Cache::new(),
            mountpoint: Mutex::new(Vec::new()),
            superblock_table: Mutex::new(SuperBlockTable::new()),
            fstype_map: BTreeMap::new(),
            root: InitedCell::uninit(),
        };

        vfs
    }

    pub fn get_root(&self) -> &Arc<Dentry> {
        &self.root
    }

    pub fn lookup_dentry(&self, dir: &Arc<Dentry>, path: &str) -> SysResult<Arc<Dentry>> {
        let mut current = match path.chars().next() {
            Some('/') => self.get_root().clone(),
            _ => dir.clone(),
        };

        // TODO: Link to
        current = current.get_mount_to();
        current = current.walk_link()?;

        path.split('/')
            .filter(|s| !(s.is_empty() || *s == "."))
            .try_for_each(|part| {
                let next = current.lookup(part)?;
                current = next.get_mount_to().walk_link()?;

                Ok(())
            })?;

        Ok(current)
    }

    pub fn lookup_parent_dentry<'a>(
        &self,
        dir: &Arc<Dentry>,
        path: &'a str,
    ) -> SysResult<(Arc<Dentry>, &'a str)> {
        let mut current = match path.chars().next() {
            Some('/') => self.get_root().clone(),
            _ => dir.clone(),
        };
        current = current.get_mount_to().walk_link()?;

        let parts: Vec<&str> = path.split('/').filter(|s| !s.is_empty()).collect();

        // if parts.is_empty() {
        //     return current.get_parent()
        //            .ok_or(Errno::ENOENT)
        //            .map(|p| (p, String::from("/")));
        // }
        if parts.is_empty() {
            return current.get_parent().ok_or(Errno::ENOENT).map(|p| (p, "/"));
        }

        for part in &parts[0..parts.len() - 1] {
            let next = current.lookup(part)?;
            current = next.get_mount_to().walk_link()?;
        }

        Ok((current, parts[parts.len() - 1]))
    }

    pub fn load_inode(&self, sno: u32, ino: u32) -> SysResult<Arc<dyn InodeOps>> {
        let index = inode::Index { sno, ino };
        if let Some(inode) = self.cache.find(&index) {
            Ok(inode)
        } else {
            let superblock_table = self.superblock_table.lock();
            let superblock = superblock_table.get(sno).ok_or(Errno::ENOENT)?;

            let inode: Arc<dyn InodeOps> = Arc::from(superblock.get_inode(ino)?);
            self.cache.insert(&index, inode.clone())?;

            Ok(inode)
        }
    }
}

unsafe impl Sync for VirtualFileSystem {}
