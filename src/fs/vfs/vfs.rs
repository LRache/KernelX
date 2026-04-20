use alloc::collections::BTreeMap;
use alloc::sync::Arc;
use alloc::vec::Vec;

use crate::fs::filesystem::FileSystemOps;
use crate::fs::inode;
use crate::fs::inode::InodeOps;
use crate::fs::perm::{Perm, PermFlags};
use crate::kernel::config;
use crate::kernel::errno::{Errno, SysResult};
use crate::klib::{InitedCell, SleepLock, SpinLock};

use super::SuperBlockTable;
use super::dentry::Dentry;

pub struct VirtualFileSystem {
    pub(super) cache: inode::Cache,
    pub(super) mountpoint: SpinLock<Vec<Arc<Dentry>>>,
    pub superblock_table: SleepLock<SuperBlockTable>,
    pub(super) fstype_map: BTreeMap<&'static str, &'static dyn FileSystemOps>,
    pub(super) root: InitedCell<Arc<Dentry>>,
}

impl VirtualFileSystem {
    pub fn new() -> Self {
        VirtualFileSystem {
            cache: inode::Cache::new(),
            mountpoint: SpinLock::new(Vec::new(), "VirtualFileSystem::mountpoint"),
            superblock_table: SleepLock::new(SuperBlockTable::new(), "VirtualFileSystem::superblock_table"),
            fstype_map: BTreeMap::new(),
            root: InitedCell::uninit(),
        }
    }

    pub fn get_root(&self) -> &Arc<Dentry> {
        &self.root
    }

    pub fn lookup_dentry(&self, dir: &Arc<Dentry>, path: &str) -> SysResult<Arc<Dentry>> {
        self.lookup_dentry_with_perm(dir, path, &Perm::current(PermFlags::X))
    }

    pub fn lookup_dentry_with_perm(&self, dir: &Arc<Dentry>, path: &str, perm: &Perm) -> SysResult<Arc<Dentry>> {
        self.lookup_dentry_with_depth_and_perm(dir, path, 0, perm)
    }

    pub(crate) fn lookup_dentry_with_depth_and_perm(
        &self,
        dir: &Arc<Dentry>,
        path: &str,
        depth: usize,
        perm: &Perm,
    ) -> SysResult<Arc<Dentry>> {
        if depth > config::MAX_SYMLINK_DEPTH {
            return Err(Errno::ELOOP);
        }

        let mut current = match path.chars().next() {
            Some('/') => self.get_root().clone(),
            _ => dir.clone(),
        };

        current = current.get_mount_to();
        current = current.walk_link_with_perm(depth, perm)?;

        path.split('/')
            .filter(|s| !(s.is_empty() || *s == "."))
            .try_for_each(|part| {
                let next = current.lookup_with_perm(part, perm)?;
                current = next.get_mount_to().walk_link_with_perm(depth, perm)?;

                Ok(())
            })?;

        Ok(current)
    }

    pub fn lookup_dentry_nofollow(&self, dir: &Arc<Dentry>, path: &str) -> SysResult<Arc<Dentry>> {
        self.lookup_dentry_nofollow_with_perm(dir, path, &Perm::current(PermFlags::X))
    }

    pub fn lookup_dentry_nofollow_with_perm(
        &self,
        dir: &Arc<Dentry>,
        path: &str,
        perm: &Perm,
    ) -> SysResult<Arc<Dentry>> {
        let current = match path.chars().next() {
            Some('/') => self.get_root().clone(),
            _ => dir.clone(),
        };

        if let Some((parent, name)) = self.lookup_parent_dentry_with_perm(dir, path, perm)? {
            Ok(parent.lookup_nocached_with_perm(name, perm)?)
        } else {
            Ok(current)
        }
    }

    pub fn lookup_parent_dentry<'a>(
        &self,
        dir: &Arc<Dentry>,
        path: &'a str,
    ) -> SysResult<Option<(Arc<Dentry>, &'a str)>> {
        self.lookup_parent_dentry_with_perm(dir, path, &Perm::current(PermFlags::X))
    }

    pub fn lookup_parent_dentry_with_perm<'a>(
        &self,
        dir: &Arc<Dentry>,
        path: &'a str,
        perm: &Perm,
    ) -> SysResult<Option<(Arc<Dentry>, &'a str)>> {
        let mut current = match path.chars().next() {
            Some('/') => self.get_root().clone(),
            _ => dir.clone(),
        };
        current = current.get_mount_to().walk_link_with_perm(0, perm)?;

        let parts: Vec<&str> = path.split('/').filter(|s| !s.is_empty()).collect();

        if parts.is_empty() {
            return Ok(current.get_parent().map(|p| (p, "/")));
        }

        // TODO: What if the path ends with `..` ?
        for part in &parts[0..parts.len() - 1] {
            if *part == "." {
                continue;
            }
            let next = current.lookup_with_perm(part, perm)?;
            current = next.get_mount_to().walk_link_with_perm(0, perm)?;
        }

        Ok(Some((current, parts[parts.len() - 1])))
    }

    pub fn load_inode(&self, sno: u32, ino: u32) -> SysResult<Arc<dyn InodeOps>> {
        let index = inode::Index { sno, ino };
        if let Some(inode) = self.cache.find(&index) {
            return Ok(inode);
        }

        let superblock = {
            let superblock_table = self.superblock_table.lock();
            superblock_table.get(sno).ok_or(Errno::ENOENT)?
        };
        let inode = superblock.get_inode(ino)?;
        self.cache.insert(&index, inode.clone())?;

        Ok(inode)
    }
}

unsafe impl Sync for VirtualFileSystem {}
