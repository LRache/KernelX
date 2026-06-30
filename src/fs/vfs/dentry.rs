use core::fmt::Debug;
#[cfg(feature = "fanotify")]
use core::sync::atomic::{AtomicUsize, Ordering};

use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::sync::{Arc, Weak};
use alloc::vec::Vec;

use crate::fs::inode::{FileType, Index, InodeOps, Mode, Owner};
use crate::fs::perm::{Perm, PermFlags};
use crate::kernel::config;
use crate::kernel::errno::{Errno, SysResult};
#[cfg(feature = "fanotify")]
use crate::kernel::event::Fanotify;
use crate::kernel::scheduler::current;
#[cfg(feature = "fanotify")]
use crate::klib::LazyInitedCell;
use crate::klib::SpinLock;

use super::{LookupFlags, vfs};

#[cfg(feature = "fanotify")]
static NEXT_MOUNT_ID: AtomicUsize = AtomicUsize::new(1);

#[derive(Clone)]
enum MountPropagation {
    Private,
    Unbindable,
    Shared(Arc<MountSharedGroup>),
    Slave(Arc<MountSharedGroup>),
}

pub(super) struct MountSharedGroup {
    peers: SpinLock<Vec<Weak<Mount>>>,
    slaves: SpinLock<Vec<Weak<Mount>>>,
}

#[derive(Clone, Copy)]
enum MountKind {
    Root,
    Filesystem,
    Bind,
}

/// Mount information for all dentries in the same mount point.
pub struct Mount {
    #[cfg(feature = "fanotify")]
    id: usize,
    kind: MountKind,

    /// The dentry where this mount is mounted. For the root mount, it is None.
    mountpoint: SpinLock<Option<Arc<Dentry>>>,

    /// The mount of the mount point. For the root mount, it is None.
    mount_parent: SpinLock<Option<Weak<Mount>>>,

    /// For bind mount
    /// The root dentry of this mount point. For bind mounts, it points to the same dentry as the mountpoint.
    root: SpinLock<Weak<Dentry>>,

    /// Root dentry in the source tree behind this mount.
    ///
    /// `root` is the dentry visible from the mount target, while `source_root`
    /// remembers where that visible tree came from. They are the same for a
    /// regular filesystem mount, but can differ for bind mounts:
    ///
    /// ```text
    /// mount --bind /real/a /dst
    /// ```
    ///
    /// In this case, `root` is the new visible root under `/dst`, and
    /// `source_root` is the original source-tree dentry `/real/a`. The
    /// propagation code uses this to translate a visible dentry back to its
    /// source-tree location before mapping the same event into shared peers;
    /// see `VirtualFileSystem::propagation_targets`.
    source_root: SpinLock<Option<Arc<Dentry>>>,

    /// For bind mount
    /// Mount propagation type. If shared, also contains the shared group information.
    propagation: SpinLock<MountPropagation>,

    /// Fanotify instance for the mount point.
    #[cfg(feature = "fanotify")]
    fanotify: LazyInitedCell<Arc<Fanotify>>,
}

pub struct Dentry {
    inode_index: Index,
    name: String,
    parent: Option<Arc<Dentry>>,
    mount_location: SpinLock<Option<DentryLocation>>,
    children: SpinLock<BTreeMap<String, Weak<Dentry>>>,
    inode: SpinLock<Weak<dyn InodeOps>>,

    /// All mounts that are mounted on this dentry. The last one is the topmost mount. For a dentry without mounts, it is empty.
    mount_stack: SpinLock<Vec<Arc<Dentry>>>,

    /// The mount this dentry belongs to.
    pub(super) mount: Arc<Mount>,
}

struct DentryLocation {
    name: String,
    parent: Option<Arc<Dentry>>,
}

impl MountSharedGroup {
    fn new() -> Self {
        Self {
            peers: SpinLock::new(Vec::new(), "MountSharedGroup::peers"),
            slaves: SpinLock::new(Vec::new(), "MountSharedGroup::slaves"),
        }
    }

    fn add_mount(list: &SpinLock<Vec<Weak<Mount>>>, mount: &Arc<Mount>) {
        let mut mounts = list.lock();
        mounts.retain(|existing| existing.upgrade().is_some());
        if mounts
            .iter()
            .filter_map(Weak::upgrade)
            .any(|existing| Arc::ptr_eq(&existing, mount))
        {
            return;
        }
        mounts.push(Arc::downgrade(mount));
    }

    fn remove_mount(list: &SpinLock<Vec<Weak<Mount>>>, mount: &Arc<Mount>) {
        list.lock().retain(|existing| match existing.upgrade() {
            Some(existing) => !Arc::ptr_eq(&existing, mount),
            None => false,
        });
    }

    fn add_peer(&self, peer: &Arc<Mount>) {
        Self::add_mount(&self.peers, peer);
    }

    fn remove_peer(&self, peer: &Arc<Mount>) {
        Self::remove_mount(&self.peers, peer);
    }

    fn add_slave(&self, slave: &Arc<Mount>) {
        Self::add_mount(&self.slaves, slave);
    }

    fn remove_slave(&self, slave: &Arc<Mount>) {
        Self::remove_mount(&self.slaves, slave);
    }

    pub(super) fn peers(&self) -> Vec<Arc<Mount>> {
        let mut peers = self.peers.lock();
        peers.retain(|peer| peer.upgrade().is_some());
        peers.iter().filter_map(Weak::upgrade).collect()
    }

    pub(super) fn slaves(&self) -> Vec<Arc<Mount>> {
        let mut slaves = self.slaves.lock();
        slaves.retain(|slave| slave.upgrade().is_some());
        slaves.iter().filter_map(Weak::upgrade).collect()
    }

    fn has_peers(&self) -> bool {
        !self.peers().is_empty()
    }

    fn take_slaves(&self) -> Vec<Arc<Mount>> {
        let mut slaves = self.slaves.lock();
        let live_slaves = slaves.iter().filter_map(Weak::upgrade).collect();
        slaves.clear();
        live_slaves
    }
}

impl Mount {
    fn new(kind: MountKind, mount_parent: Option<&Arc<Mount>>, mountpoint: Option<&Arc<Dentry>>) -> Self {
        Self {
            #[cfg(feature = "fanotify")]
            id: NEXT_MOUNT_ID.fetch_add(1, Ordering::Relaxed),
            kind,
            mount_parent: SpinLock::new(mount_parent.map(Arc::downgrade), "Mount::mount_parent"),
            mountpoint: SpinLock::new(mountpoint.cloned(), "Mount::mountpoint"),
            root: SpinLock::new(Weak::new(), "Mount::root"),
            source_root: SpinLock::new(None, "Mount::source_root"),
            propagation: SpinLock::new(MountPropagation::Private, "Mount::propagation"),
            #[cfg(feature = "fanotify")]
            fanotify: LazyInitedCell::new("Mount::fanotify"),
        }
    }

    #[cfg(feature = "fanotify")]
    pub fn id(&self) -> usize {
        self.id
    }

    /// Whether this mount has a superblock, not a bind mount.
    pub(super) fn owns_superblock(&self) -> bool {
        matches!(self.kind, MountKind::Root | MountKind::Filesystem)
    }

    pub(super) fn is_descendant_of(&self, ancestor: &Arc<Mount>) -> bool {
        let mut mount = self.parent();
        let mut visited = Vec::new();
        while let Some(current) = mount {
            if Arc::ptr_eq(&current, ancestor) {
                return true;
            }
            if visited.iter().any(|visited| Arc::ptr_eq(visited, &current)) {
                return false;
            }
            visited.push(current.clone());
            mount = current.parent();
        }
        false
    }
    pub(super) fn parent(&self) -> Option<Arc<Mount>> {
        self.mount_parent.lock().as_ref().and_then(Weak::upgrade)
    }

    pub(super) fn mountpoint(&self) -> Option<Arc<Dentry>> {
        self.mountpoint.lock().clone()
    }

    pub(super) fn set_mount_context(&self, mount_parent: &Arc<Mount>, mountpoint: &Arc<Dentry>) {
        *self.mount_parent.lock() = Some(Arc::downgrade(mount_parent));
        *self.mountpoint.lock() = Some(mountpoint.clone());
    }

    pub(super) fn root(&self) -> Option<Arc<Dentry>> {
        self.root.lock().upgrade()
    }

    fn set_root(&self, root: &Arc<Dentry>) {
        *self.root.lock() = Arc::downgrade(root);
    }

    pub(super) fn source_root(&self) -> Option<Arc<Dentry>> {
        self.source_root.lock().clone()
    }

    fn set_source_root(&self, root: &Arc<Dentry>) {
        *self.source_root.lock() = Some(root.clone());
    }

    pub(super) fn clear_source_root(&self) {
        *self.source_root.lock() = None;
    }

    pub(super) fn shared_group(&self) -> Option<Arc<MountSharedGroup>> {
        match &*self.propagation.lock() {
            MountPropagation::Private | MountPropagation::Unbindable => None,
            MountPropagation::Shared(group) => Some(group.clone()),
            MountPropagation::Slave(_) => None,
        }
    }

    pub(super) fn master_group(&self) -> Option<Arc<MountSharedGroup>> {
        match &*self.propagation.lock() {
            MountPropagation::Private | MountPropagation::Unbindable | MountPropagation::Shared(_) => None,
            MountPropagation::Slave(group) => Some(group.clone()),
        }
    }

    pub(super) fn is_shared(&self) -> bool {
        matches!(&*self.propagation.lock(), MountPropagation::Shared(_))
    }

    pub(super) fn is_unbindable(&self) -> bool {
        matches!(&*self.propagation.lock(), MountPropagation::Unbindable)
    }

    pub(super) fn make_shared(self: &Arc<Self>) -> Arc<MountSharedGroup> {
        let mut propagation = self.propagation.lock();
        let group = match &*propagation {
            MountPropagation::Private | MountPropagation::Unbindable => {
                let group = Arc::new(MountSharedGroup::new());
                *propagation = MountPropagation::Shared(group.clone());
                group
            }
            MountPropagation::Shared(group) => group.clone(),
            MountPropagation::Slave(old_group) => {
                old_group.remove_slave(self);
                let group = Arc::new(MountSharedGroup::new());
                *propagation = MountPropagation::Shared(group.clone());
                group
            }
        };
        group.add_peer(self);
        group
    }

    pub(super) fn join_shared_group(self: &Arc<Self>, group: &Arc<MountSharedGroup>) {
        let mut orphaned_slaves = None;
        let mut propagation = self.propagation.lock();
        match &*propagation {
            MountPropagation::Private | MountPropagation::Unbindable => {}
            MountPropagation::Shared(old_group) if Arc::ptr_eq(old_group, group) => return,
            MountPropagation::Shared(old_group) => {
                let old_group = old_group.clone();
                old_group.remove_peer(self);
                if !old_group.has_peers() {
                    orphaned_slaves = Some((old_group.clone(), old_group.take_slaves()));
                }
            }
            MountPropagation::Slave(old_group) => old_group.remove_slave(self),
        }
        *propagation = MountPropagation::Shared(group.clone());
        group.add_peer(self);
        drop(propagation);

        Self::make_orphaned_slaves_private(orphaned_slaves);
    }

    pub(super) fn make_slave(self: &Arc<Self>) {
        let mut orphaned_slaves = None;
        let mut propagation = self.propagation.lock();
        if let MountPropagation::Shared(group) = &*propagation {
            let group = group.clone();
            group.remove_peer(self);
            if group.has_peers() {
                group.add_slave(self);
                *propagation = MountPropagation::Slave(group);
            } else {
                orphaned_slaves = Some((group.clone(), group.take_slaves()));
                *propagation = MountPropagation::Private;
            }
        }
        drop(propagation);

        Self::make_orphaned_slaves_private(orphaned_slaves);
    }

    pub(super) fn make_slave_of(self: &Arc<Self>, group: &Arc<MountSharedGroup>) {
        let mut orphaned_slaves = None;
        let mut propagation = self.propagation.lock();
        match &*propagation {
            MountPropagation::Private | MountPropagation::Unbindable => {}
            MountPropagation::Shared(old_group) => {
                let old_group = old_group.clone();
                old_group.remove_peer(self);
                if !old_group.has_peers() {
                    orphaned_slaves = Some((old_group.clone(), old_group.take_slaves()));
                }
            }
            MountPropagation::Slave(old_group) => old_group.remove_slave(self),
        }
        if group.has_peers() {
            *propagation = MountPropagation::Slave(group.clone());
            group.add_slave(self);
        } else {
            *propagation = MountPropagation::Private;
        }
        drop(propagation);

        Self::make_orphaned_slaves_private(orphaned_slaves);
    }

    pub(super) fn make_private(self: &Arc<Self>) {
        let mut orphaned_slaves = None;
        let mut propagation = self.propagation.lock();
        match &*propagation {
            MountPropagation::Private | MountPropagation::Unbindable => {}
            MountPropagation::Shared(group) => {
                let group = group.clone();
                group.remove_peer(self);
                if !group.has_peers() {
                    orphaned_slaves = Some((group.clone(), group.take_slaves()));
                }
            }
            MountPropagation::Slave(group) => group.remove_slave(self),
        }
        *propagation = MountPropagation::Private;
        drop(propagation);

        Self::make_orphaned_slaves_private(orphaned_slaves);
    }

    pub(super) fn make_unbindable(self: &Arc<Self>) {
        let mut orphaned_slaves = None;
        let mut propagation = self.propagation.lock();
        match &*propagation {
            MountPropagation::Private | MountPropagation::Unbindable => {}
            MountPropagation::Shared(group) => {
                let group = group.clone();
                group.remove_peer(self);
                if !group.has_peers() {
                    orphaned_slaves = Some((group.clone(), group.take_slaves()));
                }
            }
            MountPropagation::Slave(group) => group.remove_slave(self),
        }
        *propagation = MountPropagation::Unbindable;
        drop(propagation);

        Self::make_orphaned_slaves_private(orphaned_slaves);
    }

    fn make_private_if_slave_of(self: &Arc<Self>, group: &Arc<MountSharedGroup>) {
        let mut propagation = self.propagation.lock();
        if let MountPropagation::Slave(master_group) = &*propagation
            && Arc::ptr_eq(master_group, group)
        {
            *propagation = MountPropagation::Private;
        }
    }

    fn make_orphaned_slaves_private(orphaned_slaves: Option<(Arc<MountSharedGroup>, Vec<Arc<Mount>>)>) {
        if let Some((group, slaves)) = orphaned_slaves {
            for slave in slaves {
                slave.make_private_if_slave_of(&group);
            }
        }
    }

    #[cfg(feature = "fanotify")]
    pub fn fanotify(&self) -> Option<Arc<Fanotify>> {
        self.fanotify.get()
    }

    /// Lazy initialize the fanotify instance for this mount point.
    /// Returns None if the superblock does not support fanotify.
    #[cfg(feature = "fanotify")]
    pub fn ensure_fanotify(&self) -> Arc<Fanotify> {
        self.fanotify.get_or_init(|| Arc::new(Fanotify::new()))
    }
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
            mount_location: SpinLock::new(None, "Dentry::mount_location"),
            children: SpinLock::new(BTreeMap::new(), "Dentry::children"),
            inode: SpinLock::new(Arc::downgrade(inode), "Dentry::inode"),
            mount_stack: SpinLock::new(Vec::new(), "Dentry::mount_stack"),
            mount: parent.mount.clone(),
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
            mount_location: SpinLock::new(None, "Dentry::mount_location"),
            children: SpinLock::new(BTreeMap::new(), "Dentry::children"),
            inode: SpinLock::new(Arc::downgrade(inode), "Dentry::inode"),
            mount_stack: SpinLock::new(Vec::new(), "Dentry::mount_stack"),
            mount: Arc::new(Mount::new(MountKind::Root, None, None)),
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

    #[cfg(feature = "fanotify")]
    pub fn superblock_fanotify(&self) -> Option<Arc<Fanotify>> {
        vfs()
            .superblock_table
            .lock()
            .get(self.sno())
            .and_then(|superblock| superblock.fanotify())
    }

    #[cfg(feature = "fanotify")]
    pub fn ensure_superblock_fanotify(&self) -> SysResult<Arc<Fanotify>> {
        vfs()
            .superblock_table
            .lock()
            .get(self.sno())
            .ok_or(Errno::ENOENT)?
            .ensure_fanotify()
            .ok_or(Errno::EOPNOTSUPP)
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

    pub fn name(&self) -> String {
        self.mount_location
            .lock()
            .as_ref()
            .map(|location| location.name.clone())
            .unwrap_or_else(|| self.name.clone())
    }

    pub fn get_parent(&self) -> Option<Arc<Dentry>> {
        self.mount_location
            .lock()
            .as_ref()
            .map(|location| location.parent.clone())
            .unwrap_or_else(|| self.parent.clone())
    }

    pub fn get_mount(&self) -> Arc<Mount> {
        self.mount.clone()
    }

    #[cfg(feature = "fanotify")]
    pub fn get_mount_id(&self) -> usize {
        self.mount.id()
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

    pub fn get_mount_to(self: Arc<Self>) -> Arc<Dentry> {
        let mut current = self;
        let mut visited = Vec::new();
        loop {
            if visited.iter().any(|visited| Arc::ptr_eq(visited, &current)) {
                return current;
            }
            visited.push(current.clone());
            let mount_to = current.mount_stack.lock().last().cloned();
            match mount_to {
                Some(mount_to) => current = mount_to,
                None => return current,
            }
        }
    }
    pub fn walk_link_with_perm(
        self: Arc<Self>,
        root: &Arc<Dentry>,
        symlink_depth: &mut usize,
        perm: &Perm,
    ) -> SysResult<Arc<Dentry>> {
        if let Some(p) = self.get_parent() {
            let inode = self.get_inode();
            if let Some(target) = inode.follow_magic_link()? {
                if *symlink_depth >= config::MAX_SYMLINK_DEPTH {
                    return Err(Errno::ELOOP);
                }
                *symlink_depth += 1;
                return Ok(target.get_mount_to());
            }

            let mut buffer = [0u8; 255];
            if let Some(length) = inode.readlink(&mut buffer)? {
                if *symlink_depth >= config::MAX_SYMLINK_DEPTH {
                    return Err(Errno::ELOOP);
                }
                *symlink_depth += 1;
                let link_name = core::str::from_utf8(&buffer[..length]).unwrap();
                let link_dentry = vfs().lookup_dentry_with_depth_and_perm(root, &p, link_name, symlink_depth, perm)?;
                return Ok(link_dentry);
            }
        }
        Ok(self)
    }

    pub fn walk_link_with_perm_and_flags(
        self: Arc<Self>,
        root: &Arc<Dentry>,
        symlink_depth: &mut usize,
        perm: &Perm,
        flags: LookupFlags,
    ) -> SysResult<Arc<Dentry>> {
        if let Some(p) = self.get_parent() {
            let inode = self.get_inode();
            if let Some(target) = inode.follow_magic_link()? {
                if *symlink_depth >= config::MAX_SYMLINK_DEPTH {
                    return Err(Errno::ELOOP);
                }
                *symlink_depth += 1;
                return Ok(target.get_mount_to());
            }

            let mut buffer = [0u8; 255];
            if let Some(length) = inode.readlink(&mut buffer)? {
                if *symlink_depth >= config::MAX_SYMLINK_DEPTH {
                    return Err(Errno::ELOOP);
                }
                *symlink_depth += 1;
                let link_name = core::str::from_utf8(&buffer[..length]).unwrap();
                let link_dentry =
                    vfs().lookup_dentry_with_depth_perm_flags(root, &p, link_name, symlink_depth, perm, flags)?;
                return Ok(link_dentry);
            }
        }
        Ok(self)
    }

    pub fn mount(self: &Arc<Self>, mount_to: &Arc<dyn InodeOps>, mount_to_sno: u32) -> Arc<Mount> {
        let (mount_parent, mountpoint) = self.new_mount_context();
        let mount = Arc::new(Mount::new(
            MountKind::Filesystem,
            Some(&mount_parent),
            Some(&mountpoint),
        ));
        let mounted_root = Arc::new(Dentry {
            inode_index: Index {
                sno: mount_to_sno,
                ino: mount_to.get_ino(),
            },
            name: mountpoint.name(),
            parent: mountpoint.get_parent(),
            mount_location: SpinLock::new(None, "Dentry::mount_location"),
            children: SpinLock::new(BTreeMap::new(), "Dentry::children"),
            inode: SpinLock::new(Arc::downgrade(mount_to), "Dentry::inode"),
            mount_stack: SpinLock::new(Vec::new(), "Dentry::mount_stack"),
            mount: mount.clone(),
        });
        mount.set_root(&mounted_root);
        mount.set_source_root(&mounted_root);
        mountpoint.mount_stack.lock().push(mounted_root);
        mount
    }

    pub fn bind_mount(self: &Arc<Self>, source: &Arc<Dentry>, source_root: &Arc<Dentry>) -> Arc<Mount> {
        let source_inode = source.get_inode();
        let (mount_parent, mountpoint) = self.new_mount_context();
        let mount = Arc::new(Mount::new(MountKind::Bind, Some(&mount_parent), Some(&mountpoint)));
        let mounted_root = Arc::new(Dentry {
            inode_index: source.get_inode_index(),
            name: mountpoint.name(),
            parent: mountpoint.get_parent(),
            mount_location: SpinLock::new(None, "Dentry::mount_location"),
            children: SpinLock::new(BTreeMap::new(), "Dentry::children"),
            inode: SpinLock::new(Arc::downgrade(&source_inode), "Dentry::inode"),
            mount_stack: SpinLock::new(Vec::new(), "Dentry::mount_stack"),
            mount: mount.clone(),
        });
        mount.set_root(&mounted_root);
        mount.set_source_root(source_root);
        mountpoint.mount_stack.lock().push(mounted_root);
        mount
    }

    pub(super) fn set_mount_location(&self, mountpoint: &Arc<Dentry>) {
        let location = DentryLocation {
            name: mountpoint.name(),
            parent: mountpoint.get_parent(),
        };
        *self.mount_location.lock() = Some(location);
    }
    pub(super) fn push_mount_root(&self, mounted_root: Arc<Dentry>) {
        self.mount_stack.lock().push(mounted_root);
    }

    pub(super) fn push_mount_root_if_top(&self, mounted_root: Arc<Dentry>, expected_top: Option<&Arc<Dentry>>) -> bool {
        let mut mount_stack = self.mount_stack.lock();
        let top_matches = match (mount_stack.last(), expected_top) {
            (Some(current), Some(expected)) => Arc::ptr_eq(current, expected),
            (None, None) => true,
            _ => false,
        };
        if top_matches {
            mount_stack.push(mounted_root);
        }
        top_matches
    }

    /// Return the parent mount of the new mount and the
    pub(super) fn new_mount_context(self: &Arc<Self>) -> (Arc<Mount>, Arc<Dentry>) {
        let mountpoint = self.clone().get_mount_to();
        if Arc::ptr_eq(&mountpoint, self) {
            (self.mount.clone(), self.clone())
        } else {
            (mountpoint.mount.clone(), mountpoint)
        }
    }

    pub fn mounted_root(self: &Arc<Self>) -> Option<Arc<Dentry>> {
        self.mount_stack.lock().last().cloned()
    }

    pub(super) fn contains_mount(&self, mount: &Arc<Mount>) -> bool {
        self.mount_stack
            .lock()
            .iter()
            .any(|mounted_root| Arc::ptr_eq(&mounted_root.mount, mount))
    }

    pub fn unmount(self: &Arc<Self>) -> Option<Arc<Dentry>> {
        self.mount_stack.lock().pop()
    }

    pub(super) fn unmount_mount(self: &Arc<Self>, mount: &Arc<Mount>) -> Option<Arc<Dentry>> {
        let mut mount_stack = self.mount_stack.lock();
        match mount_stack.last() {
            Some(mounted_root) if Arc::ptr_eq(&mounted_root.mount, mount) => mount_stack.pop(),
            _ => None,
        }
    }

    pub fn get_path(&self) -> String {
        let name = self.name();
        if let Some(parent) = self.get_parent() {
            let mut path = parent.get_path();
            if !path.ends_with('/') {
                path.push('/');
            }
            if name != "/" {
                path.push_str(&name);
            }
            path
        } else {
            name
        }
    }
    fn create_with(
        self: &Arc<Self>,
        name: &str,
        mode: Mode,
        owner: Owner,
        create: impl FnOnce(&Arc<dyn InodeOps>, &str, Mode, Owner) -> SysResult<Arc<dyn InodeOps>>,
    ) -> SysResult<Arc<dyn InodeOps>> {
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

        let inherited_sgid = parent_mode.contains(Mode::S_ISGID) && (mode & Mode::S_IFMT) == Mode::S_IFDIR;
        if parent_mode.contains(Mode::S_ISGID) {
            owner.gid = parent_gid;
            if inherited_sgid {
                mode.insert(Mode::S_ISGID);
            }
        }

        if mode.contains(Mode::S_ISGID) && current::fsuid() != 0 && !inherited_sgid {
            let pcb = current::pcb();
            let in_supplementary_group = pcb.supplementary_gids().contains(&owner.gid);
            if pcb.fsgid() != owner.gid && !in_supplementary_group {
                mode.remove(Mode::S_ISGID);
            }
        }

        let inode = create(&parent_inode, name, mode, owner)?;
        let index = Index {
            sno: self.sno(),
            ino: inode.get_ino(),
        };

        Ok(vfs().cache.get_or_insert(index, inode))
    }

    pub fn create(self: &Arc<Self>, name: &str, mode: Mode, owner: Owner) -> SysResult<Arc<dyn InodeOps>> {
        self.create_with(name, mode, owner, |parent_inode, name, mode, owner| {
            parent_inode.create(name, mode, owner)
        })
    }

    pub fn mknod(self: &Arc<Self>, name: &str, mode: Mode, owner: Owner, dev: u64) -> SysResult<Arc<dyn InodeOps>> {
        self.create_with(name, mode, owner, |parent_inode, name, mode, owner| {
            parent_inode.mknod(name, mode, owner, dev)
        })
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

        child_inode.sync()?;
        if remove_dir {
            parent_inode.rmdir(name)?;
        } else {
            parent_inode.unlink(name)?;
        }

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
        let old_inode = vfs().load_inode(self.sno(), old_ino)?;
        let old_is_dir = old_inode.inode_type()? == FileType::Directory;
        let new_parent_inode = new_parent.get_inode();
        let overwritten = match new_parent_inode.lookup(new_name) {
            Ok(ino) if ino != old_ino => {
                let overwritten_inode = vfs().load_inode(new_parent.sno(), ino)?;
                if old_is_dir && overwritten_inode.inode_type()? == FileType::Directory {
                    let mut index = 0;
                    while let Some((dent, next_index)) = overwritten_inode.get_dent(index)? {
                        if dent.name != "." && dent.name != ".." {
                            return Err(Errno::ENOTEMPTY);
                        }
                        index = next_index;
                    }
                }

                overwritten_inode.sync()?;
                Some(Index {
                    sno: new_parent.sno(),
                    ino,
                })
            }
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
        self.check_search_perm(&Perm::current(PermFlags::X))?;

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
            self.name()
        )
    }
}
