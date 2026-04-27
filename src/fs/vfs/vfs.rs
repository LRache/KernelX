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

use super::SuperBlockTable;
use super::dentry::Dentry;

bitflags! {
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub struct LookupFlags: u32 {
        const NO_XDEV = 1;
    }
}

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
        let mut symlink_depth = 0;
        self.lookup_dentry_with_depth_and_perm(dir, path, &mut symlink_depth, perm)
    }

    pub(crate) fn lookup_dentry_with_depth_and_perm(
        &self,
        dir: &Arc<Dentry>,
        path: &str,
        symlink_depth: &mut usize,
        perm: &Perm,
    ) -> SysResult<Arc<Dentry>> {
        let mut current = match path.chars().next() {
            Some('/') => self.get_root().clone(),
            _ => dir.clone(),
        };

        current = current.get_mount_to();
        current = current.walk_link_with_perm(symlink_depth, perm)?;

        for part in path.split('/').filter(|s| !(s.is_empty() || *s == ".")) {
            if part == ".." {
                current = Self::lookup_parent_component(current, perm, LookupFlags::empty())?;
                continue;
            }

            let next = current.lookup_with_perm(part, perm)?;
            current = next.get_mount_to().walk_link_with_perm(symlink_depth, perm)?;
        }

        Ok(current)
    }

    pub(crate) fn lookup_dentry_with_depth_perm_flags(
        &self,
        dir: &Arc<Dentry>,
        path: &str,
        symlink_depth: &mut usize,
        perm: &Perm,
        flags: LookupFlags,
    ) -> SysResult<Arc<Dentry>> {
        let mut current = match path.chars().next() {
            Some('/') => self.get_root().clone(),
            _ => dir.clone(),
        };

        current = self.follow_mount(current, flags)?;
        current = current.walk_link_with_perm_and_flags(symlink_depth, perm, flags)?;

        for part in path.split('/').filter(|s| !(s.is_empty() || *s == ".")) {
            if part == ".." {
                current = Self::lookup_parent_component(current, perm, flags)?;
                continue;
            }

            let next = current.lookup_with_perm(part, perm)?;
            current = self
                .follow_mount(next, flags)?
                .walk_link_with_perm_and_flags(symlink_depth, perm, flags)?;
        }

        Ok(current)
    }

    pub fn lookup_dentry_with_flags(
        &self,
        dir: &Arc<Dentry>,
        path: &str,
        flags: LookupFlags,
    ) -> SysResult<Arc<Dentry>> {
        self.lookup_dentry_with_perm_and_flags(dir, path, &Perm::current(PermFlags::X), flags)
    }

    pub fn lookup_dentry_with_perm_and_flags(
        &self,
        dir: &Arc<Dentry>,
        path: &str,
        perm: &Perm,
        flags: LookupFlags,
    ) -> SysResult<Arc<Dentry>> {
        let mut symlink_depth = 0;
        self.lookup_dentry_with_depth_perm_flags(dir, path, &mut symlink_depth, perm, flags)
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
        let mut symlink_depth = 0;
        let current = match path.chars().next() {
            Some('/') => self.get_root().clone(),
            _ => dir.clone(),
        }
        .get_mount_to()
        .walk_link_with_perm(&mut symlink_depth, perm)?;

        if let Some((parent, name)) =
            self.lookup_parent_dentry_with_depth_and_perm(dir, path, &mut symlink_depth, perm)?
        {
            let dentry = parent.lookup_nocached_with_perm(name.as_ref(), perm)?;
            Ok(dentry.get_mount_to())
        } else {
            Ok(current)
        }
    }

    pub fn lookup_dentry_nofollow_with_perm_and_flags(
        &self,
        dir: &Arc<Dentry>,
        path: &str,
        perm: &Perm,
        flags: LookupFlags,
    ) -> SysResult<Arc<Dentry>> {
        let mut symlink_depth = 0;
        let current = match path.chars().next() {
            Some('/') => self.get_root().clone(),
            _ => dir.clone(),
        };
        let current =
            self.follow_mount(current, flags)?
                .walk_link_with_perm_and_flags(&mut symlink_depth, perm, flags)?;

        if let Some((parent, name)) =
            self.lookup_parent_dentry_with_depth_perm_flags(dir, path, &mut symlink_depth, perm, flags)?
        {
            let dentry = parent.lookup_nocached_with_perm(name.as_ref(), perm)?;
            self.follow_mount(dentry, flags)
        } else {
            Ok(current)
        }
    }

    pub fn lookup_parent_dentry<'a>(
        &self,
        dir: &Arc<Dentry>,
        path: &'a str,
    ) -> SysResult<Option<(Arc<Dentry>, Cow<'a, str>)>> {
        self.lookup_parent_dentry_with_perm(dir, path, &Perm::current(PermFlags::X))
    }

    pub fn lookup_parent_dentry_with_perm<'a>(
        &self,
        dir: &Arc<Dentry>,
        path: &'a str,
        perm: &Perm,
    ) -> SysResult<Option<(Arc<Dentry>, Cow<'a, str>)>> {
        let mut symlink_depth = 0;
        self.lookup_parent_dentry_with_depth_and_perm(dir, path, &mut symlink_depth, perm)
    }

    pub(crate) fn lookup_parent_dentry_with_depth_and_perm<'a>(
        &self,
        dir: &Arc<Dentry>,
        path: &'a str,
        symlink_depth: &mut usize,
        perm: &Perm,
    ) -> SysResult<Option<(Arc<Dentry>, Cow<'a, str>)>> {
        let mut current = match path.chars().next() {
            Some('/') => self.get_root().clone(),
            _ => dir.clone(),
        };
        current = current.get_mount_to().walk_link_with_perm(symlink_depth, perm)?;

        let parts: Vec<&str> = path.split('/').filter(|s| !s.is_empty()).collect();

        if parts.is_empty() {
            return Ok(current.get_parent().map(|p| (p, Cow::Borrowed("/"))));
        }

        for part in &parts[0..parts.len() - 1] {
            if *part == "." {
                continue;
            }
            if *part == ".." {
                current = Self::lookup_parent_component(current, perm, LookupFlags::empty())?;
                continue;
            }
            let next = current.lookup_with_perm(part, perm)?;
            current = next.get_mount_to().walk_link_with_perm(symlink_depth, perm)?;
        }

        let name = parts[parts.len() - 1];
        if name != "." && name != ".." {
            return Ok(Some((current, Cow::Borrowed(name))));
        }

        let target = if name == "." {
            current.check_search_perm(perm)?;
            current
        } else {
            Self::lookup_parent_component(current, perm, LookupFlags::empty())?
        };

        Ok(target
            .get_parent()
            .map(|parent| (parent, Cow::Owned(target.name().into()))))
    }

    pub(crate) fn lookup_parent_dentry_with_depth_perm_flags<'a>(
        &self,
        dir: &Arc<Dentry>,
        path: &'a str,
        symlink_depth: &mut usize,
        perm: &Perm,
        flags: LookupFlags,
    ) -> SysResult<Option<(Arc<Dentry>, Cow<'a, str>)>> {
        let mut current = match path.chars().next() {
            Some('/') => self.get_root().clone(),
            _ => dir.clone(),
        };
        current = self
            .follow_mount(current, flags)?
            .walk_link_with_perm_and_flags(symlink_depth, perm, flags)?;

        let parts: Vec<&str> = path.split('/').filter(|s| !s.is_empty()).collect();

        if parts.is_empty() {
            return Ok(current.get_parent().map(|p| (p, Cow::Borrowed("/"))));
        }

        for part in &parts[0..parts.len() - 1] {
            if *part == "." {
                continue;
            }
            if *part == ".." {
                current = Self::lookup_parent_component(current, perm, flags)?;
                continue;
            }
            let next = current.lookup_with_perm(part, perm)?;
            current = self
                .follow_mount(next, flags)?
                .walk_link_with_perm_and_flags(symlink_depth, perm, flags)?;
        }

        let name = parts[parts.len() - 1];
        if name != "." && name != ".." {
            return Ok(Some((current, Cow::Borrowed(name))));
        }

        let target = if name == "." {
            current.check_search_perm(perm)?;
            current
        } else {
            Self::lookup_parent_component(current, perm, flags)?
        };

        Ok(target
            .get_parent()
            .map(|parent| (parent, Cow::Owned(target.name().into()))))
    }

    pub fn lookup_parent_dentry_with_flags<'a>(
        &self,
        dir: &Arc<Dentry>,
        path: &'a str,
        flags: LookupFlags,
    ) -> SysResult<Option<(Arc<Dentry>, Cow<'a, str>)>> {
        self.lookup_parent_dentry_with_perm_and_flags(dir, path, &Perm::current(PermFlags::X), flags)
    }

    pub fn lookup_parent_dentry_with_perm_and_flags<'a>(
        &self,
        dir: &Arc<Dentry>,
        path: &'a str,
        perm: &Perm,
        flags: LookupFlags,
    ) -> SysResult<Option<(Arc<Dentry>, Cow<'a, str>)>> {
        let mut symlink_depth = 0;
        self.lookup_parent_dentry_with_depth_perm_flags(dir, path, &mut symlink_depth, perm, flags)
    }

    fn follow_mount(&self, dentry: Arc<Dentry>, flags: LookupFlags) -> SysResult<Arc<Dentry>> {
        if flags.contains(LookupFlags::NO_XDEV) && dentry.mounted_root().is_some() {
            return Err(Errno::EXDEV);
        }
        Ok(dentry.get_mount_to())
    }

    fn lookup_parent_component(current: Arc<Dentry>, perm: &Perm, flags: LookupFlags) -> SysResult<Arc<Dentry>> {
        current.check_search_perm(perm)?;

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
