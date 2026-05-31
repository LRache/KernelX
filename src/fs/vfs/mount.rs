use alloc::string::{String, ToString};
use alloc::sync::Arc;
use alloc::vec::Vec;

use crate::driver::BlockDriverOps;
use crate::fs::filesystem::{FileSystemOps, MountOptions};
use crate::fs::inode::FileType;
use crate::fs::vfs::vfs::VirtualFileSystem;
use crate::kernel::errno::{Errno, SysResult};

use super::dentry::{Dentry, Mount};
use super::{split_path, vfs};

impl VirtualFileSystem {
    fn mount(
        &self,
        root: &Arc<Dentry>,
        dir: &Arc<Dentry>,
        path: &str,
        fstype: &'static dyn FileSystemOps,
        device: Option<Arc<dyn BlockDriverOps>>,
        options: MountOptions,
    ) -> SysResult<()> {
        let dentry = self.resolve_mountpoint(root, dir, path)?; // The dentry will be used as the mount point. 
        let (parent_mount, parent_mountpoint) = dentry.new_mount_context();
        let parent_is_shared = parent_mount.is_shared();
        let propagation_targets = self.propagation_targets(&parent_mount, &parent_mountpoint)?;

        // Prepare the superblock and the root inode.
        let (sno, root_ino) = {
            let mut superblock_table = self.superblock_table.lock();
            let sno = superblock_table.mount(fstype, device, options)?;
            (sno, superblock_table.get(sno).unwrap().get_root_ino())
        };

        let root_inode = self.load_inode(sno, root_ino)?;

        let mount = dentry.mount(&root_inode, sno);
        let mounted_root = dentry.mounted_root().ok_or(Errno::EINVAL)?;
        let propagated_mounts = self.bind_mount_targets(&mounted_root, &propagation_targets);
        if parent_is_shared {
            Self::share_mounts(&mount, &propagated_mounts);
        }

        let mut mounts = self.mounts.lock();
        mounts.push(mount);
        mounts.extend(propagated_mounts);

        Ok(())
    }

    fn bind_mount(&self, root: &Arc<Dentry>, dir: &Arc<Dentry>, source: &str, target: &str) -> SysResult<()> {
        let source = self.resolve_source(root, dir, source)?;
        let target = self.resolve_mountpoint(root, dir, target)?;
        let source_group = source.get_mount().shared_group();
        let (parent_mount, parent_mountpoint) = target.new_mount_context();
        let parent_is_shared = parent_mount.is_shared();
        let propagation_targets = self.propagation_targets(&parent_mount, &parent_mountpoint)?;
        let source_is_dir = source.get_inode().inode_type()? == FileType::Directory;
        Self::check_bind_target_type(source_is_dir, &target)?;
        for propagation_target in &propagation_targets {
            Self::check_bind_target_type(source_is_dir, propagation_target)?;
        }

        let mount = target.bind_mount(&source);
        let propagated_mounts = self.bind_mount_targets(&source, &propagation_targets);
        if let Some(group) = source_group {
            mount.join_shared_group(&group);
            for propagated_mount in &propagated_mounts {
                propagated_mount.join_shared_group(&group);
            }
        } else if parent_is_shared {
            Self::share_mounts(&mount, &propagated_mounts);
        }

        let mut mounts = self.mounts.lock();
        mounts.push(mount);
        mounts.extend(propagated_mounts);

        Ok(())
    }

    fn check_bind_target_type(source_is_dir: bool, target: &Arc<Dentry>) -> SysResult<()> {
        let target_is_dir = target.get_inode().inode_type()? == FileType::Directory;
        if source_is_dir && !target_is_dir {
            return Err(Errno::ENOTDIR);
        }
        if !source_is_dir && target_is_dir {
            return Err(Errno::EISDIR);
        }
        Ok(())
    }

    fn bind_mount_targets(&self, source: &Arc<Dentry>, targets: &[Arc<Dentry>]) -> Vec<Arc<Mount>> {
        targets.iter().map(|target| target.bind_mount(source)).collect()
    }

    fn share_mounts(mount: &Arc<Mount>, propagated_mounts: &[Arc<Mount>]) {
        let group = mount.make_shared();
        for propagated_mount in propagated_mounts {
            propagated_mount.join_shared_group(&group);
        }
    }

    fn propagation_targets(&self, parent_mount: &Arc<Mount>, mountpoint: &Arc<Dentry>) -> SysResult<Vec<Arc<Dentry>>> {
        let Some(group) = parent_mount.shared_group() else {
            return Ok(Vec::new());
        };
        let relative_path = Self::relative_path_from_mount(parent_mount, mountpoint)?;
        let mut targets = Vec::new();
        for peer in group.peers() {
            if Arc::ptr_eq(&peer, parent_mount) {
                continue;
            }
            if relative_path.is_empty() {
                if let Some(mountpoint) = peer.mountpoint() {
                    targets.push(mountpoint);
                }
                continue;
            }
            let Some(peer_root) = peer.root() else {
                continue;
            };
            targets.push(Self::lookup_relative_mountpoint(peer_root, &relative_path)?);
        }
        Ok(targets)
    }

    fn relative_path_from_mount(mount: &Arc<Mount>, dentry: &Arc<Dentry>) -> SysResult<Vec<String>> {
        let root = mount.root().ok_or(Errno::EINVAL)?;
        let mut current = Some(dentry.clone());
        let mut path = Vec::new();

        while let Some(dentry) = current {
            if Arc::ptr_eq(&dentry, &root) {
                path.reverse();
                return Ok(path);
            }
            path.push(dentry.name().into());
            current = dentry.get_parent();
        }

        Err(Errno::EINVAL)
    }

    fn lookup_relative_mountpoint(mut current: Arc<Dentry>, path: &[String]) -> SysResult<Arc<Dentry>> {
        for name in path {
            current = current.lookup(name)?;
        }
        Ok(current)
    }

    /// Resolve the mount point for the given path.
    /// If the path is a mount point, return the mount point dentry. Otherwise, return the dentry for the path.
    fn resolve_mountpoint(&self, root: &Arc<Dentry>, dir: &Arc<Dentry>, path: &str) -> SysResult<Arc<Dentry>> {
        if let Some(mountpoint) = self.lookup_mountpoint_by_path(&Self::absolute_path(dir, path)) {
            return Ok(mountpoint);
        }
        let dentry = match self.lookup_parent_dentry(root, dir, path)? {
            Some((parent, name)) => parent.lookup(name.as_ref()).or_else(|err| {
                if err != Errno::ENOENT {
                    return Err(err);
                }
                self.lookup_mountpoint_by_path(&Self::join_path(&parent, name.as_ref()))
                    .ok_or(Errno::ENOENT)
            }),
            None => Ok(root.clone()),
        }?;
        Ok(self.resolve_mountpoint_alias(dentry))
    }

    fn resolve_source(&self, root: &Arc<Dentry>, dir: &Arc<Dentry>, path: &str) -> SysResult<Arc<Dentry>> {
        Ok(self.resolve_mountpoint(root, dir, path)?.get_mount_to())
    }

    fn resolve_mountpoint_alias(&self, dentry: Arc<Dentry>) -> Arc<Dentry> {
        if dentry.mounted_root().is_some() {
            return dentry;
        }
        let path = dentry.get_path();
        self.lookup_mountpoint_by_path(&path).unwrap_or(dentry)
    }

    pub(super) fn lookup_mountpoint_by_path(&self, path: &str) -> Option<Arc<Dentry>> {
        self.mounts
            .lock()
            .iter()
            .rev()
            .filter(|mount| Self::is_mount_record_active(mount))
            .filter_map(|mount| mount.mountpoint())
            .find(|mountpoint| mountpoint.get_path() == path)
    }

    pub(super) fn join_path(parent: &Arc<Dentry>, name: &str) -> String {
        if name == "/" {
            return parent.get_path();
        }
        let mut path = parent.get_path();
        if !path.ends_with('/') {
            path.push('/');
        }
        path.push_str(name);
        path
    }

    fn absolute_path(dir: &Arc<Dentry>, path: &str) -> String {
        let mut parts = if path.starts_with('/') {
            Vec::new()
        } else {
            split_path(&dir.get_path())
                .filter(|part| *part != ".")
                .map(str::to_string)
                .collect()
        };
        for part in split_path(path) {
            if part == "." {
                continue;
            }
            if part == ".." {
                parts.pop();
                continue;
            }
            parts.push(part.to_string());
        }
        if parts.is_empty() {
            "/".into()
        } else {
            let mut absolute = String::from("/");
            absolute.push_str(&parts.join("/"));
            absolute
        }
    }

    fn remount(&self, root: &Arc<Dentry>, dir: &Arc<Dentry>, path: &str, options: MountOptions) -> SysResult<()> {
        let dentry = self.resolve_mountpoint(root, dir, path)?;
        let mounted_root = dentry.mounted_root().ok_or(Errno::EINVAL)?;
        self.superblock_table.lock().remount(mounted_root.sno(), options)
    }

    fn make_shared(&self, root: &Arc<Dentry>, dir: &Arc<Dentry>, path: &str, recursive: bool) -> SysResult<()> {
        let dentry = self.resolve_mountpoint(root, dir, path)?;
        let mount = dentry.mounted_root().ok_or(Errno::EINVAL)?.get_mount();
        mount.make_shared();

        if recursive {
            let mounts = self.mounts.lock().clone();
            for shared_mount in mounts {
                if !shared_mount.is_descendant_of(&mount) {
                    continue;
                }
                shared_mount.make_shared();
            }
        }

        Ok(())
    }

    fn make_private(&self, root: &Arc<Dentry>, dir: &Arc<Dentry>, path: &str, recursive: bool) -> SysResult<()> {
        let dentry = self.resolve_mountpoint(root, dir, path)?;
        let mount = dentry.mounted_root().ok_or(Errno::EINVAL)?.get_mount();
        mount.make_private();

        if recursive {
            let mounts = self.mounts.lock().clone();
            for private_mount in mounts {
                if !private_mount.is_descendant_of(&mount) {
                    continue;
                }
                private_mount.make_private();
            }
        }

        Ok(())
    }

    fn unmount(&self, root: &Arc<Dentry>, dir: &Arc<Dentry>, path: &str) -> SysResult<()> {
        let dentry = self.resolve_mountpoint(root, dir, path)?;
        if dentry.mounted_root().is_none() {
            return Err(Errno::EINVAL);
        }
        let mount_path = dentry.get_path();
        let mounted_root = dentry.mounted_root().ok_or(Errno::EINVAL)?;
        let mounted_mount = mounted_root.get_mount();
        let parent_mount = mounted_mount.parent().ok_or(Errno::EINVAL)?;
        let mountpoint = mounted_mount.mountpoint().ok_or(Errno::EINVAL)?;
        let mut mountpoints = Vec::new();
        mountpoints.push(dentry);
        let propagated_mountpoints = self.propagation_targets(&parent_mount, &mountpoint)?;
        mountpoints.extend(propagated_mountpoints);

        let mut targets = Vec::new();
        for mountpoint in mountpoints {
            let Some(mounted_root) = mountpoint.mounted_root() else {
                continue;
            };
            let mount = mounted_root.get_mount();
            if targets
                .iter()
                .any(|(_, target_mount, _, _)| Arc::ptr_eq(target_mount, &mount))
            {
                continue;
            }
            let mounted_sno = mounted_root.sno();
            let owns_superblock = mount.owns_superblock();
            targets.push((mountpoint, mount, mounted_sno, owns_superblock));
        }
        if targets.is_empty() {
            return Err(Errno::EINVAL);
        }

        let mounts = self.mounts.lock().clone();
        if let Some(descendant) = mounts.iter().find(|mounted| {
            if !Self::is_mount_record_active(mounted) {
                return false;
            }
            if let Some(mountpoint) = mounted.mountpoint()
                && targets
                    .iter()
                    .any(|(target_mountpoint, _, _, _)| mountpoint.get_path() == target_mountpoint.get_path())
            {
                return false;
            }
            !targets
                .iter()
                .any(|(_, target_mount, _, _)| Arc::ptr_eq(mounted, target_mount))
                && targets
                    .iter()
                    .any(|(_, target_mount, _, _)| mounted.is_descendant_of(target_mount))
        }) {
            let descendant_path = descendant
                .mountpoint()
                .map(|mountpoint| mountpoint.get_path())
                .unwrap_or_else(|| "<unknown>".into());
            // crate::kinfo!(
            //     "Unmount failed: mountpoint {} has descendant mountpoint {}",
            //     mount_path,
            //     descendant_path
            // );
            return Err(Errno::EBUSY);
        }

        for (_, _, mounted_sno, owns_superblock) in &targets {
            if *owns_superblock && self.cache.superblock_busy(*mounted_sno) {
                crate::kinfo!(
                    "Unmount failed: superblock {} is busy, type={}",
                    mounted_sno,
                    self.superblock_table.lock().get(*mounted_sno).unwrap().type_name()
                );
                return Err(Errno::EBUSY);
            }
        }

        for (mountpoint, mount, _, _) in &targets {
            let mounted_root = mountpoint.mounted_root().ok_or(Errno::EINVAL)?;
            if !Arc::ptr_eq(&mounted_root.get_mount(), mount) {
                return Err(Errno::EINVAL);
            }
        }
        for (mountpoint, mount, _, _) in &targets {
            mountpoint.unmount_mount(mount).ok_or(Errno::EINVAL)?;
            mount.make_private();
        }
        self.mounts.lock().retain(|mounted| {
            Self::is_mount_record_active(mounted)
                && !targets
                    .iter()
                    .any(|(_, target_mount, _, _)| Arc::ptr_eq(mounted, target_mount))
        });
        for (_, _, mounted_sno, owns_superblock) in targets {
            if owns_superblock {
                self.cache.remove_superblock(mounted_sno);
                self.superblock_table.lock().unmount(mounted_sno)?;
            }
        }

        Ok(())
    }

    pub fn mountpoint_list(&self) -> Vec<Arc<Dentry>> {
        self.mounts
            .lock()
            .iter()
            .filter(|mount| Self::is_mount_record_active(mount))
            .filter_map(|mount| mount.mountpoint())
            .collect()
    }

    pub(super) fn is_mount_record_active(mount: &Arc<Mount>) -> bool {
        match mount.mountpoint() {
            Some(mountpoint) => mountpoint.contains_mount(mount),
            None => false,
        }
    }
}

pub fn mount(
    root: &Arc<Dentry>,
    dir: &Arc<Dentry>,
    path: &str,
    fstype: &'static dyn FileSystemOps,
    device: Option<Arc<dyn BlockDriverOps>>,
    options: MountOptions,
) -> Result<(), Errno> {
    vfs().mount(root, dir, path, fstype, device, options)
}

pub fn bind_mount(root: &Arc<Dentry>, dir: &Arc<Dentry>, source: &str, target: &str) -> Result<(), Errno> {
    vfs().bind_mount(root, dir, source, target)
}

pub fn unmount(root: &Arc<Dentry>, dir: &Arc<Dentry>, path: &str) -> Result<(), Errno> {
    vfs().unmount(root, dir, path)
}

pub fn remount(root: &Arc<Dentry>, dir: &Arc<Dentry>, path: &str, options: MountOptions) -> Result<(), Errno> {
    vfs().remount(root, dir, path, options)
}

pub fn make_shared(root: &Arc<Dentry>, dir: &Arc<Dentry>, path: &str, recursive: bool) -> Result<(), Errno> {
    vfs().make_shared(root, dir, path, recursive)
}

pub fn make_private(root: &Arc<Dentry>, dir: &Arc<Dentry>, path: &str, recursive: bool) -> Result<(), Errno> {
    vfs().make_private(root, dir, path, recursive)
}

pub fn unmount_all() -> SysResult<()> {
    let vfs = vfs();
    let _ = vfs.cache.sync();
    let mounts = vfs.mounts.lock().clone();
    for mount in mounts {
        if let Some(mountpoint) = mount.mountpoint() {
            mountpoint.unmount();
        }
    }
    vfs.mounts.lock().clear();
    vfs.cache.clear();
    let mut superblock_table = vfs.superblock_table.lock();
    superblock_table.unmount_all()?;

    Ok(())
}
