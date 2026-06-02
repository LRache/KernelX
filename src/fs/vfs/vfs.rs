use alloc::borrow::Cow;
use alloc::collections::BTreeMap;
use alloc::sync::Arc;
use alloc::vec::Vec;
use bitflags::bitflags;

use crate::fs::filesystem::FileSystemOps;
use crate::fs::inode;
use crate::fs::inode::InodeOps;
use crate::fs::perm::{Perm, PermFlags};
use crate::kernel::errno::{Errno, SysResult};
use crate::klib::{InitedCell, SleepLock, SpinLock};

use super::dentry::{Dentry, Mount};
use super::{SuperBlockTable, split_path};

bitflags! {
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub struct LookupFlags: u32 {
        const NO_XDEV = 1;
    }
}

pub struct VirtualFileSystem {
    pub(super) cache: inode::Cache,
    pub(super) mounts: SpinLock<Vec<Arc<Mount>>>,
    pub superblock_table: SleepLock<SuperBlockTable>,
    pub(super) fstype_map: BTreeMap<&'static str, &'static dyn FileSystemOps>,
    pub(super) root: InitedCell<Arc<Dentry>>,
}

impl VirtualFileSystem {
    pub fn new() -> Self {
        VirtualFileSystem {
            cache: inode::Cache::new(),
            mounts: SpinLock::new(Vec::new(), "VirtualFileSystem::mounts"),
            superblock_table: SleepLock::new(SuperBlockTable::new(), "VirtualFileSystem::superblock_table"),
            fstype_map: BTreeMap::new(),
            root: InitedCell::uninit(),
        }
    }

    pub fn get_root(&self) -> &Arc<Dentry> {
        &self.root
    }

    pub fn lookup_dentry(&self, root: &Arc<Dentry>, dir: &Arc<Dentry>, path: &str) -> SysResult<Arc<Dentry>> {
        self.lookup_dentry_with_perm(root, dir, path, &Perm::current(PermFlags::X))
    }

    pub fn lookup_dentry_with_perm(
        &self,
        root: &Arc<Dentry>,
        dir: &Arc<Dentry>,
        path: &str,
        perm: &Perm,
    ) -> SysResult<Arc<Dentry>> {
        let mut symlink_depth = 0;
        self.lookup_dentry_with_depth_and_perm(root, dir, path, &mut symlink_depth, perm)
    }

    pub fn lookup_dentry_with_depth_and_perm(
        &self,
        root: &Arc<Dentry>,
        dir: &Arc<Dentry>,
        path: &str,
        symlink_depth: &mut usize,
        perm: &Perm,
    ) -> SysResult<Arc<Dentry>> {
        let mut current = match path.chars().next() {
            Some('/') => root.clone(),
            _ => dir.clone(),
        };

        current = current.get_mount_to();
        current = current.walk_link_with_perm(root, symlink_depth, perm)?;

        for part in split_path(path).filter(|part| *part != ".") {
            if part == ".." {
                current = Self::lookup_parent_component(root, current, perm, LookupFlags::empty())?;
                continue;
            }

            let next = self.lookup_child_with_mount_alias(&current, part, perm)?;
            current = next.get_mount_to().walk_link_with_perm(root, symlink_depth, perm)?;
        }

        Ok(current)
    }

    pub fn lookup_dentry_with_depth_perm_flags(
        &self,
        root: &Arc<Dentry>,
        dir: &Arc<Dentry>,
        path: &str,
        symlink_depth: &mut usize,
        perm: &Perm,
        flags: LookupFlags,
    ) -> SysResult<Arc<Dentry>> {
        let mut current = match path.chars().next() {
            Some('/') => root.clone(),
            _ => dir.clone(),
        };

        current = self.follow_mount(current, flags)?;
        current = current.walk_link_with_perm_and_flags(root, symlink_depth, perm, flags)?;

        for part in split_path(path).filter(|part| *part != ".") {
            if part == ".." {
                current = Self::lookup_parent_component(root, current, perm, flags)?;
                continue;
            }

            let next = self.lookup_child_with_mount_alias(&current, part, perm)?;
            current =
                self.follow_mount(next, flags)?
                    .walk_link_with_perm_and_flags(root, symlink_depth, perm, flags)?;
        }

        Ok(current)
    }

    pub fn lookup_dentry_with_flags(
        &self,
        root: &Arc<Dentry>,
        dir: &Arc<Dentry>,
        path: &str,
        flags: LookupFlags,
    ) -> SysResult<Arc<Dentry>> {
        self.lookup_dentry_with_perm_and_flags(root, dir, path, &Perm::current(PermFlags::X), flags)
    }

    pub fn lookup_dentry_with_perm_and_flags(
        &self,
        root: &Arc<Dentry>,
        dir: &Arc<Dentry>,
        path: &str,
        perm: &Perm,
        flags: LookupFlags,
    ) -> SysResult<Arc<Dentry>> {
        let mut symlink_depth = 0;
        self.lookup_dentry_with_depth_perm_flags(root, dir, path, &mut symlink_depth, perm, flags)
    }

    pub fn lookup_dentry_nofollow(&self, root: &Arc<Dentry>, dir: &Arc<Dentry>, path: &str) -> SysResult<Arc<Dentry>> {
        self.lookup_dentry_nofollow_with_perm(root, dir, path, &Perm::current(PermFlags::X))
    }

    pub fn lookup_dentry_nofollow_with_perm(
        &self,
        root: &Arc<Dentry>,
        dir: &Arc<Dentry>,
        path: &str,
        perm: &Perm,
    ) -> SysResult<Arc<Dentry>> {
        let mut symlink_depth = 0;
        let current = match path.chars().next() {
            Some('/') => root.clone(),
            _ => dir.clone(),
        }
        .get_mount_to()
        .walk_link_with_perm(root, &mut symlink_depth, perm)?;

        if let Some((parent, name)) =
            self.lookup_parent_dentry_with_depth_and_perm(root, dir, path, &mut symlink_depth, perm)?
        {
            let dentry = self.lookup_child_with_mount_alias(&parent, name.as_ref(), perm)?;
            Ok(dentry.get_mount_to())
        } else {
            Ok(current)
        }
    }

    pub fn lookup_dentry_nofollow_with_perm_and_flags(
        &self,
        root: &Arc<Dentry>,
        dir: &Arc<Dentry>,
        path: &str,
        perm: &Perm,
        flags: LookupFlags,
    ) -> SysResult<Arc<Dentry>> {
        let mut symlink_depth = 0;
        let current = match path.chars().next() {
            Some('/') => root.clone(),
            _ => dir.clone(),
        };
        let current =
            self.follow_mount(current, flags)?
                .walk_link_with_perm_and_flags(root, &mut symlink_depth, perm, flags)?;

        if let Some((parent, name)) =
            self.lookup_parent_dentry_with_depth_perm_flags(root, dir, path, &mut symlink_depth, perm, flags)?
        {
            let dentry = self.lookup_child_with_mount_alias(&parent, name.as_ref(), perm)?;
            self.follow_mount(dentry, flags)
        } else {
            Ok(current)
        }
    }

    pub fn lookup_parent_dentry<'a>(
        &self,
        root: &Arc<Dentry>,
        dir: &Arc<Dentry>,
        path: &'a str,
    ) -> SysResult<Option<(Arc<Dentry>, Cow<'a, str>)>> {
        self.lookup_parent_dentry_with_perm(root, dir, path, &Perm::current(PermFlags::X))
    }

    pub fn lookup_parent_dentry_with_perm<'a>(
        &self,
        root: &Arc<Dentry>,
        dir: &Arc<Dentry>,
        path: &'a str,
        perm: &Perm,
    ) -> SysResult<Option<(Arc<Dentry>, Cow<'a, str>)>> {
        let mut symlink_depth = 0;
        self.lookup_parent_dentry_with_depth_and_perm(root, dir, path, &mut symlink_depth, perm)
    }

    pub(crate) fn lookup_parent_dentry_with_depth_and_perm<'a>(
        &self,
        root: &Arc<Dentry>,
        dir: &Arc<Dentry>,
        path: &'a str,
        symlink_depth: &mut usize,
        perm: &Perm,
    ) -> SysResult<Option<(Arc<Dentry>, Cow<'a, str>)>> {
        let mut current = match path.chars().next() {
            Some('/') => root.clone(),
            _ => dir.clone(),
        };
        current = current.get_mount_to().walk_link_with_perm(root, symlink_depth, perm)?;

        let parts: Vec<&str> = split_path(path).collect();

        if parts.is_empty() {
            if Arc::ptr_eq(&current, root) {
                return Ok(None);
            }
            return Ok(current.get_parent().map(|p| (p, Cow::Borrowed("/"))));
        }

        for part in &parts[0..parts.len() - 1] {
            if *part == "." {
                continue;
            }
            if *part == ".." {
                current = Self::lookup_parent_component(root, current, perm, LookupFlags::empty())?;
                continue;
            }
            let next = self.lookup_child_with_mount_alias(&current, part, perm)?;
            current = next.get_mount_to().walk_link_with_perm(root, symlink_depth, perm)?;
        }

        let name = parts[parts.len() - 1];
        if name != "." && name != ".." {
            return Ok(Some((current, Cow::Borrowed(name))));
        }

        let target = if name == "." {
            current.check_search_perm(perm)?;
            current
        } else {
            Self::lookup_parent_component(root, current, perm, LookupFlags::empty())?
        };

        if Arc::ptr_eq(&target, root) {
            return Ok(None);
        }

        Ok(target.get_parent().map(|parent| (parent, Cow::Owned(target.name()))))
    }

    pub(crate) fn lookup_parent_dentry_with_depth_perm_flags<'a>(
        &self,
        root: &Arc<Dentry>,
        dir: &Arc<Dentry>,
        path: &'a str,
        symlink_depth: &mut usize,
        perm: &Perm,
        flags: LookupFlags,
    ) -> SysResult<Option<(Arc<Dentry>, Cow<'a, str>)>> {
        let mut current = match path.chars().next() {
            Some('/') => root.clone(),
            _ => dir.clone(),
        };
        current = self
            .follow_mount(current, flags)?
            .walk_link_with_perm_and_flags(root, symlink_depth, perm, flags)?;

        let parts: Vec<&str> = split_path(path).collect();

        if parts.is_empty() {
            if Arc::ptr_eq(&current, root) {
                return Ok(None);
            }
            return Ok(current.get_parent().map(|p| (p, Cow::Borrowed("/"))));
        }

        for part in &parts[0..parts.len() - 1] {
            if *part == "." {
                continue;
            }
            if *part == ".." {
                current = Self::lookup_parent_component(root, current, perm, flags)?;
                continue;
            }
            let next = self.lookup_child_with_mount_alias(&current, part, perm)?;
            current =
                self.follow_mount(next, flags)?
                    .walk_link_with_perm_and_flags(root, symlink_depth, perm, flags)?;
        }

        let name = parts[parts.len() - 1];
        if name != "." && name != ".." {
            return Ok(Some((current, Cow::Borrowed(name))));
        }

        let target = if name == "." {
            current.check_search_perm(perm)?;
            current
        } else {
            Self::lookup_parent_component(root, current, perm, flags)?
        };

        if Arc::ptr_eq(&target, root) {
            return Ok(None);
        }

        Ok(target.get_parent().map(|parent| (parent, Cow::Owned(target.name()))))
    }

    pub fn lookup_parent_dentry_with_flags<'a>(
        &self,
        root: &Arc<Dentry>,
        dir: &Arc<Dentry>,
        path: &'a str,
        flags: LookupFlags,
    ) -> SysResult<Option<(Arc<Dentry>, Cow<'a, str>)>> {
        self.lookup_parent_dentry_with_perm_and_flags(root, dir, path, &Perm::current(PermFlags::X), flags)
    }

    pub fn lookup_parent_dentry_with_perm_and_flags<'a>(
        &self,
        root: &Arc<Dentry>,
        dir: &Arc<Dentry>,
        path: &'a str,
        perm: &Perm,
        flags: LookupFlags,
    ) -> SysResult<Option<(Arc<Dentry>, Cow<'a, str>)>> {
        let mut symlink_depth = 0;
        self.lookup_parent_dentry_with_depth_perm_flags(root, dir, path, &mut symlink_depth, perm, flags)
    }

    fn follow_mount(&self, dentry: Arc<Dentry>, flags: LookupFlags) -> SysResult<Arc<Dentry>> {
        if flags.contains(LookupFlags::NO_XDEV) && dentry.mounted_root().is_some() {
            return Err(Errno::EXDEV);
        }
        Ok(dentry.get_mount_to())
    }

    fn lookup_child_with_mount_alias(&self, parent: &Arc<Dentry>, name: &str, perm: &Perm) -> SysResult<Arc<Dentry>> {
        parent.lookup_with_perm(name, perm)
    }

    fn lookup_parent_component(
        root: &Arc<Dentry>,
        current: Arc<Dentry>,
        perm: &Perm,
        flags: LookupFlags,
    ) -> SysResult<Arc<Dentry>> {
        current.check_search_perm(perm)?;
        if Arc::ptr_eq(&current, root) {
            return Ok(current);
        }

        let current_sno = current.sno();
        let parent = current.get_parent().unwrap_or(current);
        if flags.contains(LookupFlags::NO_XDEV) && parent.sno() != current_sno {
            return Err(Errno::EXDEV);
        }

        Ok(parent)
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
