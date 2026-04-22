use core::fmt::Debug;

use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::sync::{Arc, Weak};

use crate::fs::inode::{FileType, Index, InodeOps, Mode, Owner};
use crate::fs::perm::{Perm, PermFlags};
use crate::kernel::config;
use crate::kernel::errno::{Errno, SysResult};
use crate::kernel::scheduler::current;
use crate::klib::SpinLock;

use super::vfs;

pub struct Dentry {
    inode_index: Index,
    name: String,
    parent: Option<Arc<Dentry>>,
    children: SpinLock<BTreeMap<String, Weak<Dentry>>>,
    inode: SpinLock<Weak<dyn InodeOps>>,
    mount_to: SpinLock<Option<Arc<Dentry>>>,
}

impl Dentry {
    pub fn check_search_perm(&self, perm: &Perm) -> SysResult<()> {
        let inode = self.get_inode();
        if inode.inode_type()? != FileType::Directory {
            return Err(Errno::ENOTDIR);
        }

        let mode = inode.mode()?;
        let (uid, gid) = inode.owner()?;
        if !mode.check_perm(perm, uid, gid) {
            return Err(Errno::EACCES);
        }

        Ok(())
    }

    fn check_child_mutation_perm(&self) -> SysResult<()> {
        let inode = self.get_inode();
        if inode.inode_type()? != FileType::Directory {
            return Err(Errno::ENOTDIR);
        }

        let mode = inode.mode()?;
        let (uid, gid) = inode.owner()?;
        if !mode.check_perm(&Perm::current(PermFlags::W | PermFlags::X), uid, gid) {
            return Err(Errno::EACCES);
        }

        Ok(())
    }

    fn check_sticky_remove_perm(
        &self,
        parent_inode: &Arc<dyn InodeOps>,
        child_inode: &Arc<dyn InodeOps>,
    ) -> SysResult<()> {
        let parent_mode = parent_inode.mode()?;
        if !parent_mode.contains(Mode::S_ISVTX) {
            return Ok(());
        }

        let fsuid = current::fsuid();
        if fsuid == 0 {
            return Ok(());
        }

        let (parent_uid, _) = parent_inode.owner()?;
        if fsuid == parent_uid {
            return Ok(());
        }

        let (child_uid, _) = child_inode.owner()?;
        if fsuid == child_uid {
            return Ok(());
        }

        Err(Errno::EPERM)
    }

    pub fn new(name: &str, parent: &Arc<Dentry>, inode: &Arc<dyn InodeOps>, sno: u32) -> Self {
        Self {
            inode_index: Index {
                sno: sno,
                ino: inode.get_ino(),
            },
            name: name.into(),
            parent: Some(parent.clone()),
            children: SpinLock::new(BTreeMap::new(), "Dentry::children"),
            inode: SpinLock::new(Arc::downgrade(inode), "Dentry::inode"),
            mount_to: SpinLock::new(None, "Dentry::mount_to"),
        }
    }

    pub fn root(inode: &Arc<dyn InodeOps>, sno: u32) -> Self {
        Self {
            inode_index: Index {
                sno,
                ino: inode.get_ino(),
            },
            name: "/".into(),
            parent: None,
            children: SpinLock::new(BTreeMap::new(), "Dentry::children"),
            inode: SpinLock::new(Arc::downgrade(inode), "Dentry::inode"),
            mount_to: SpinLock::new(None, "Dentry::mount_to"),
        }
    }

    pub fn sno(&self) -> u32 {
        self.inode_index.sno
    }

    pub fn ino(&self) -> u32 {
        self.inode_index.ino
    }

    pub fn get_inode_index(&self) -> Index {
        self.inode_index
    }

    pub fn is_superblock_readonly(&self) -> SysResult<bool> {
        vfs().is_superblock_readonly(self.sno())
    }

    pub fn get_inode(&self) -> Arc<dyn InodeOps> {
        let inode = self.inode.lock();
        match inode.upgrade() {
            None => {
                drop(inode);
                let inode = vfs().load_inode(self.sno(), self.ino()).expect("Failed to load inode");
                *self.inode.lock() = Arc::downgrade(&inode);
                inode
            }
            Some(inode) => inode,
        }
    }

    pub(super) fn name(&self) -> &str {
        &self.name
    }

    pub fn get_parent(&self) -> Option<Arc<Dentry>> {
        self.parent.clone()
    }

    pub fn lookup_with_perm(self: &Arc<Self>, name: &str, perm: &Perm) -> SysResult<Arc<Dentry>> {
        self.check_search_perm(perm)?;

        if let Some(child) = self.children.lock().get(name)
            && let Some(child) = child.upgrade()
        {
            return Ok(child);
        }

        let lookup_ino = self.get_inode().lookup(name)?;
        let lookup_sno = self.sno();
        let inode = vfs().load_inode(lookup_sno, lookup_ino)?;

        let new_child = Arc::new(Self::new(name, self, &inode, lookup_sno));

        let mut children = self.children.lock();
        if let Some(existing_child) = children.get(name)
            && existing_child.upgrade().is_some()
        {
            Ok(existing_child.upgrade().unwrap())
        } else {
            children.insert(name.into(), Arc::downgrade(&new_child));
            Ok(new_child)
        }
    }

    pub fn lookup(self: &Arc<Self>, name: &str) -> SysResult<Arc<Dentry>> {
        self.lookup_with_perm(name, &Perm::current(PermFlags::X))
    }

    pub fn lookup_nocached_with_perm(self: &Arc<Self>, name: &str, perm: &Perm) -> SysResult<Arc<Dentry>> {
        self.check_search_perm(perm)?;

        let lookup_ino = self.get_inode().lookup(name)?;
        let lookup_sno = self.sno();
        let inode = vfs().load_inode(lookup_sno, lookup_ino)?;

        let new_child = Arc::new(Self::new(name, self, &inode, lookup_sno));

        Ok(new_child)
    }

    pub fn get_mount_to(self: Arc<Self>) -> Arc<Dentry> {
        if let Some(mount_to) = &*self.mount_to.lock() {
            mount_to.clone()
        } else {
            self
        }
    }

    pub fn walk_link_with_perm(self: Arc<Self>, symlink_depth: &mut usize, perm: &Perm) -> SysResult<Arc<Dentry>> {
        if let Some(p) = self.parent.as_ref() {
            let inode = self.get_inode();
            let mut buffer = [0u8; 255];
            if let Some(length) = inode.readlink(&mut buffer)? {
                if *symlink_depth >= config::MAX_SYMLINK_DEPTH {
                    return Err(Errno::ELOOP);
                }
                *symlink_depth += 1;
                let link_name = core::str::from_utf8(&buffer[..length]).unwrap();
                let link_dentry = vfs().lookup_dentry_with_depth_and_perm(p, link_name, symlink_depth, perm)?;
                return Ok(link_dentry);
            }
        }
        Ok(self)
    }

    pub fn mount(self: &Arc<Self>, mount_to: &Arc<dyn InodeOps>, mount_to_sno: u32) {
        *self.mount_to.lock() = Some(Arc::new(Dentry {
            inode_index: Index {
                sno: mount_to_sno,
                ino: mount_to.get_ino(),
            },
            name: self.name.clone(),
            parent: self.parent.clone(),
            children: SpinLock::new(BTreeMap::new(), "Dentry::children"),
            inode: SpinLock::new(Arc::downgrade(mount_to), "Dentry::inode"),
            mount_to: SpinLock::new(None, "Dentry::mount_to"),
        }));
    }

    pub fn mounted_root(self: &Arc<Self>) -> Option<Arc<Dentry>> {
        self.mount_to.lock().clone()
    }

    pub fn unmount(self: &Arc<Self>) -> Option<Arc<Dentry>> {
        self.mount_to.lock().take()
    }

    pub fn get_path(&self) -> String {
        if let Some(parent) = self.parent.as_ref() {
            let mut path = parent.get_path();
            if !path.ends_with('/') {
                path.push('/');
            }
            if self.name != "/" {
                path.push_str(&self.name);
            }
            path
        } else {
            self.name.clone()
        }
    }

    pub fn create(self: &Arc<Self>, name: &str, mode: Mode, owner: Owner) -> SysResult<Arc<dyn InodeOps>> {
        self.check_child_mutation_perm()?;

        match self.lookup(name) {
            Ok(_) => return Err(Errno::EEXIST),
            Err(Errno::ENOENT) => {}
            Err(err) => return Err(err),
        }

        let parent_inode = self.get_inode();
        let parent_mode = parent_inode.mode()?;
        let parent_gid = parent_inode.owner()?.1;
        let mut mode = mode;
        let mut owner = owner;

        if parent_mode.contains(Mode::S_ISGID) {
            owner.gid = parent_gid;
            if mode.contains(Mode::S_IFDIR) {
                mode.insert(Mode::S_ISGID);
            }
        }

        if mode.contains(Mode::S_ISGID) && current::fsuid() != 0 {
            let pcb = current::pcb();
            let in_supplementary_group = pcb.supplementary_gids().contains(&owner.gid);
            if pcb.fsgid() != owner.gid && !in_supplementary_group {
                mode.remove(Mode::S_ISGID);
            }
        }

        let inode = parent_inode.create(name, mode, owner)?;
        vfs().cache.insert(
            &Index {
                sno: self.sno(),
                ino: inode.get_ino(),
            },
            inode.clone(),
        )?;

        Ok(inode)
    }

    fn remove_child(self: &Arc<Self>, name: &str, remove_dir: bool) -> SysResult<()> {
        self.check_child_mutation_perm()?;
        let child = self.lookup(name)?;

        let parent_inode = self.get_inode();
        if parent_inode.inode_type()? != FileType::Directory {
            return Err(Errno::ENOTDIR);
        }

        if remove_dir && child.mounted_root().is_some() {
            return Err(Errno::EBUSY);
        }

        let child_inode = child.get_inode();
        let child_is_dir = child_inode.inode_type()? == FileType::Directory;
        if remove_dir && !child_is_dir {
            return Err(Errno::ENOTDIR);
        }
        if !remove_dir && child_is_dir {
            return Err(Errno::EISDIR);
        }

        self.check_sticky_remove_perm(&parent_inode, &child_inode)?;

        parent_inode.unlink(name)?;

        self.children.lock().remove(name);
        vfs().cache.remove(&child.get_inode_index());

        Ok(())
    }

    pub fn unlink(self: &Arc<Self>, name: &str) -> SysResult<()> {
        self.remove_child(name, false)
    }

    pub fn rmdir(self: &Arc<Self>, name: &str) -> SysResult<()> {
        self.remove_child(name, true)
    }

    pub fn create_symlink(self: &Arc<Self>, name: &str, target: &str, owner: Owner) -> SysResult<()> {
        let inode = self.create(name, Mode::S_IFLNK | Mode::from_bits_truncate(0o777), owner)?;
        inode.symlink(target)
    }

    pub fn symlink(self: &Arc<Self>, target: &str) -> SysResult<()> {
        let inode = self.get_inode();

        inode.symlink(target)
    }

    pub fn link(self: &Arc<Self>, name: &str, target: &Arc<Dentry>) -> SysResult<()> {
        self.check_child_mutation_perm()?;

        match self.lookup(name) {
            Ok(_) => return Err(Errno::EEXIST),
            Err(Errno::ENOENT) => {}
            Err(err) => return Err(err),
        }

        let target_inode = target.get_inode();
        self.get_inode().link(name, &target_inode)?;
        vfs().cache.insert(
            &Index {
                sno: self.sno(),
                ino: target_inode.get_ino(),
            },
            target_inode,
        )?;

        Ok(())
    }

    pub fn rename(self: &Arc<Self>, old_name: &str, new_parent: &Arc<Dentry>, new_name: &str) -> SysResult<()> {
        debug_assert!(self.sno() == new_parent.sno());
        debug_assert!(old_name != "." && old_name != "..");
        debug_assert!(new_name != "." && new_name != "..");
        if Arc::ptr_eq(self, new_parent) && old_name == new_name {
            return Ok(());
        }

        let old_parent_inode = self.get_inode();
        let old_ino = old_parent_inode.lookup(old_name)?;
        let new_parent_inode = new_parent.get_inode();
        let overwritten = match new_parent_inode.lookup(new_name) {
            Ok(ino) if ino != old_ino => Some(Index {
                sno: new_parent.sno(),
                ino,
            }),
            Ok(_) | Err(Errno::ENOENT) => None,
            Err(err) => return Err(err),
        };

        old_parent_inode.rename(old_name, &new_parent_inode, new_name)?;

        self.children.lock().remove(old_name);
        new_parent.children.lock().remove(new_name);
        if let Some(index) = overwritten {
            vfs().cache.remove(&index);
        }

        Ok(())
    }

    pub fn readlink(&self, child: &str, buf: &mut [u8]) -> SysResult<Option<usize>> {
        let lookup_ino = self.get_inode().lookup(child)?;
        let lookup_sno = self.sno();
        let inode = vfs().load_inode(lookup_sno, lookup_ino)?;
        inode.readlink(buf)
    }
}

impl Debug for Dentry {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(
            f,
            "Dentry {{ sno: {}, ino: {}, name: {} }}",
            self.sno(),
            self.ino(),
            self.name
        )
    }
}
