use alloc::string::String;
use alloc::sync::Arc;
use alloc::vec::Vec;

use crate::driver::BlockDriverOps;
use crate::fs::filesystem::{FileSystemOps, MountOptions};
use crate::fs::inode::FileType;
use crate::fs::vfs::vfs::VirtualFileSystem;
use crate::kernel::errno::{Errno, SysResult};

use super::dentry::{Dentry, Mount, MountSharedGroup};
use super::vfs;

#[derive(Clone, Copy, PartialEq, Eq)]
enum PropagationTargetKind {
    Peer,
    Slave,
}

struct PropagationTarget {
    dentry: Arc<Dentry>,
    kind: PropagationTargetKind,
}

struct PropagatedMount {
    mount: Arc<Mount>,
    kind: PropagationTargetKind,
}

struct RecursiveBindSource {
    mount: Arc<Mount>,
    source: Arc<Dentry>,
    source_root: Arc<Dentry>,
    relative_path: Vec<String>,
    is_dir: bool,
}

impl VirtualFileSystem {
    /// Resolve the mount point for the given path.
    /// If the path is a mount point, return the mount point dentry. Otherwise, return the dentry for the path.
    ///
    /// ```bash
    /// mount -t tmpfs none /mnt
    /// ```
    /// Self::resolve_mountpoint("/mnt") -> dentry for orignal "/mnt" before walking into the mounted tmpfs, which is the mount point.
    ///
    /// Dentry::get_mount_to("/mnt") -> dentry for the root of the mounted tmpfs, after walking into the mounted tmpfs.
    fn resolve_mountpoint(&self, root: &Arc<Dentry>, dir: &Arc<Dentry>, path: &str) -> SysResult<Arc<Dentry>> {
        let dentry = match self.lookup_parent_dentry(root, dir, path)? {
            Some((parent, name)) => parent.lookup(name.as_ref()),
            None => Ok(root.clone()),
        }?;
        Self::resolve_top_mountpoint(dentry)
    }

    fn resolve_source(&self, root: &Arc<Dentry>, dir: &Arc<Dentry>, path: &str) -> SysResult<Arc<Dentry>> {
        Ok(self.resolve_mountpoint(root, dir, path)?.get_mount_to())
    }

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
        let (parent_mount, mountpoint) = dentry.new_mount_context();
        let parent_is_shared = parent_mount.is_shared();
        let propagation_targets = self.propagation_targets(&parent_mount, &mountpoint)?;

        // Prepare the superblock and the root inode.
        let (sno, root_ino) = {
            let mut superblock_table = self.superblock_table.lock();
            let sno = superblock_table.mount(fstype, device, options)?;
            (sno, superblock_table.get(sno).unwrap().get_root_ino())
        };

        let root_inode = self.load_inode(sno, root_ino)?;

        let mount = dentry.mount(&root_inode, sno);
        let mounted_root = mount.root().ok_or(Errno::EINVAL)?;
        let propagated_mounts = self.bind_mount_targets(&mounted_root, &mounted_root, &propagation_targets);
        if parent_is_shared {
            Self::share_mounts(&mount, &propagated_mounts);
        }

        // The new mount record should be added to the global mount list for later lookup when resolving mount points.
        let mut mounts = self.mounts.lock();
        mounts.push(mount);
        mounts.extend(propagated_mounts.into_iter().map(|propagated| propagated.mount));

        Ok(())
    }

    fn bind_mount(
        &self,
        root: &Arc<Dentry>,
        dir: &Arc<Dentry>,
        source: &str,
        target: &str,
        recursive: bool,
    ) -> SysResult<()> {
        // The mount source dentry should is the visible dentry after following all mounts on the source path,
        // whose `Dentry::mount` will be cloned to the target mountpoint.
        let source = self.resolve_source(root, dir, source)?;

        // The mount target dentry should be the original dentry before following any mounts on the target path,
        // the new mount will be pushed into whose `Dentry::mount_stack`.
        let target = self.resolve_mountpoint(root, dir, target)?;

        let source_mount = &source.mount;
        if source_mount.is_unbindable() {
            return Err(Errno::EINVAL);
        }
        let source_group = source_mount.shared_group();
        let source_master_group = source_mount.master_group();

        let source_root = Self::source_dentry_from_mount(source_mount, &source)?;
        let recursive_sources = if recursive {
            self.collect_recursive_bind_sources(&source)?
        } else {
            Vec::new()
        };

        let (parent_mount, mountpoint) = target.new_mount_context();
        let parent_is_shared = parent_mount.is_shared();
        let propagation_targets = self.propagation_targets(&parent_mount, &mountpoint)?;
        let source_is_dir = source.get_inode().inode_type()? == FileType::Directory;
        Self::check_bind_target_type(source_is_dir, &target)?;
        for propagation_target in &propagation_targets {
            Self::check_bind_target_type(source_is_dir, &propagation_target.dentry)?;
        }

        let mount = target.bind_mount(&source, &source_root);
        let propagated_mounts = self.bind_mount_targets(&source, &source_root, &propagation_targets);
        let mountpoint = mount.mountpoint().ok_or(Errno::EINVAL)?;
        if let Some(group) = source_group {
            Self::join_mounts_to_group(&mount, &propagated_mounts, &group);
        } else if let Some(group) = source_master_group {
            mount.make_slave_of(&group);
            for propagated_mount in &propagated_mounts {
                propagated_mount.mount.make_slave_of(&group);
            }
        } else if parent_is_shared {
            Self::share_mounts(&mount, &propagated_mounts);
        }

        let mut new_mounts = Vec::new();
        new_mounts.push(mount);
        new_mounts.extend(propagated_mounts.iter().map(|propagated| propagated.mount.clone()));

        if recursive {
            match self.bind_recursive_mounts(&recursive_sources, &mountpoint) {
                Ok(recursive_mounts) => new_mounts.extend(recursive_mounts),
                Err(err) => {
                    Self::rollback_mounts(&new_mounts);
                    return Err(err);
                }
            }

            for propagated_mount in &propagated_mounts {
                let target = propagated_mount.mount.mountpoint().ok_or(Errno::EINVAL)?;
                match self.bind_recursive_mounts(&recursive_sources, &target) {
                    Ok(recursive_mounts) => new_mounts.extend(recursive_mounts),
                    Err(err) => {
                        Self::rollback_mounts(&new_mounts);
                        return Err(err);
                    }
                }
            }
        }

        self.mounts.lock().extend(new_mounts);

        Ok(())
    }

    fn move_mount(&self, root: &Arc<Dentry>, dir: &Arc<Dentry>, source: &str, target: &str) -> SysResult<()> {
        let source_mountpoint = self.resolve_mountpoint(root, dir, source)?;
        let Some(mounted_root) = source_mountpoint.mounted_root() else {
            return Err(Errno::EINVAL);
        };
        let mount = mounted_root.get_mount();
        let source_parent = mount.parent().ok_or(Errno::EINVAL)?;
        let source_mountpoint = mount.mountpoint().ok_or(Errno::EINVAL)?;
        if source_parent.is_shared() {
            return Err(Errno::EINVAL);
        }

        let target = self.resolve_mountpoint(root, dir, target)?;
        if Arc::ptr_eq(&source_mountpoint, &target) {
            return Ok(());
        }

        let source_is_dir = mounted_root.get_inode().inode_type()? == FileType::Directory;
        Self::check_bind_target_type(source_is_dir, &target)?;

        let (target_parent, target_mountpoint) = target.new_mount_context();
        let target_parent_is_shared = target_parent.is_shared();
        if Arc::ptr_eq(&target_parent, &mount) || target_parent.is_descendant_of(&mount) {
            return Err(Errno::EINVAL);
        }
        if Self::mount_chain_contains(&mounted_root, &target_mountpoint) {
            return Err(Errno::EINVAL);
        }

        if !Arc::ptr_eq(&source_mountpoint.mounted_root().ok_or(Errno::EINVAL)?, &mounted_root) {
            return Err(Errno::EINVAL);
        }

        let propagation_targets = if target_parent_is_shared {
            self.propagation_targets(&target_parent, &target_mountpoint)?
        } else {
            Vec::new()
        };
        for propagation_target in &propagation_targets {
            Self::check_bind_target_type(source_is_dir, &propagation_target.dentry)?;
            if Self::mount_chain_contains(&mounted_root, &propagation_target.dentry) {
                return Err(Errno::EINVAL);
            }
        }

        let source_root = mount.source_root().ok_or(Errno::EINVAL)?;
        let recursive_sources = if propagation_targets.is_empty() {
            Vec::new()
        } else {
            self.collect_recursive_bind_sources(&mounted_root)?
        };
        let propagated_mounts = self.bind_mount_targets(&mounted_root, &source_root, &propagation_targets);
        let mut new_mounts = propagated_mounts
            .iter()
            .map(|propagated| propagated.mount.clone())
            .collect::<Vec<_>>();
        for propagated_mount in &propagated_mounts {
            let target = propagated_mount.mount.mountpoint().ok_or(Errno::EINVAL)?;
            match self.bind_recursive_mounts(&recursive_sources, &target) {
                Ok(recursive_mounts) => new_mounts.extend(recursive_mounts),
                Err(err) => {
                    Self::rollback_mounts(&new_mounts);
                    return Err(err);
                }
            }
        }
        let target_top = target_mountpoint.mounted_root();
        if let Err(err) = source_mountpoint.unmount_mount(&mount).ok_or(Errno::EINVAL) {
            Self::rollback_mounts(&new_mounts);
            return Err(err);
        }
        mount.set_mount_context(&target_parent, &target_mountpoint);
        mounted_root.set_mount_location(&target_mountpoint);
        if !target_mountpoint.push_mount_root_if_top(mounted_root.clone(), target_top.as_ref()) {
            mount.set_mount_context(&source_parent, &source_mountpoint);
            mounted_root.set_mount_location(&source_mountpoint);
            source_mountpoint.push_mount_root(mounted_root);
            Self::rollback_mounts(&new_mounts);
            return Err(Errno::EBUSY);
        }

        if let Some(group) = mount.shared_group() {
            Self::join_mounts_to_group(&mount, &propagated_mounts, &group);
        } else if let Some(group) = mount.master_group() {
            mount.make_slave_of(&group);
            for propagated_mount in &propagated_mounts {
                propagated_mount.mount.make_slave_of(&group);
            }
        } else if target_parent_is_shared {
            Self::share_mounts(&mount, &propagated_mounts);
        }

        self.mounts.lock().extend(new_mounts);

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

    fn bind_mount_targets(
        &self,
        source: &Arc<Dentry>,
        source_root: &Arc<Dentry>,
        targets: &[PropagationTarget],
    ) -> Vec<PropagatedMount> {
        targets
            .iter()
            .map(|target| PropagatedMount {
                mount: target.dentry.bind_mount(source, source_root),
                kind: target.kind,
            })
            .collect()
    }

    fn share_mounts(mount: &Arc<Mount>, propagated_mounts: &[PropagatedMount]) {
        let group = mount.make_shared();
        Self::join_mounts_to_group(mount, propagated_mounts, &group);
    }

    fn join_mounts_to_group(mount: &Arc<Mount>, propagated_mounts: &[PropagatedMount], group: &Arc<MountSharedGroup>) {
        mount.join_shared_group(group);
        for propagated_mount in propagated_mounts {
            match propagated_mount.kind {
                PropagationTargetKind::Peer => propagated_mount.mount.join_shared_group(group),
                PropagationTargetKind::Slave => propagated_mount.mount.make_slave_of(group),
            }
        }
    }

    fn rollback_mounts(mounts: &[Arc<Mount>]) {
        for mount in mounts.iter().rev() {
            mount.make_private();
            mount.clear_source_root();
            if let Some(mountpoint) = mount.mountpoint() {
                let _ = mountpoint.unmount_mount(mount);
            }
        }
    }

    fn collect_recursive_bind_sources(&self, source: &Arc<Dentry>) -> SysResult<Vec<RecursiveBindSource>> {
        let mounts = self.mounts.lock().clone();
        let source_mount = source.get_mount();
        let mut recursive_sources = Vec::new();
        for (index, mount) in mounts.into_iter().enumerate() {
            if !Self::is_mount_record_active(&mount) {
                continue;
            }
            let Some(mountpoint) = mount.mountpoint() else {
                continue;
            };
            let Some(relative_path) = Self::relative_path_from_dentry(source, &mountpoint) else {
                continue;
            };
            if relative_path.is_empty() {
                continue;
            }
            let mut current = Some(mount.clone());
            let mut skip_unbindable = false;
            while let Some(current_mount) = current {
                if Arc::ptr_eq(&current_mount, &source_mount) {
                    break;
                }
                if current_mount.is_unbindable() {
                    skip_unbindable = true;
                    break;
                }
                current = current_mount.parent();
            }
            if skip_unbindable {
                continue;
            }
            let mounted_root = mount.root().ok_or(Errno::EINVAL)?;
            let source_root = mount.source_root().ok_or(Errno::EINVAL)?;
            let is_dir = mounted_root.get_inode().inode_type()? == FileType::Directory;
            recursive_sources.push((
                index,
                RecursiveBindSource {
                    mount,
                    source: mounted_root,
                    source_root,
                    relative_path,
                    is_dir,
                },
            ));
        }
        recursive_sources.sort_by(|(left_index, left), (right_index, right)| {
            left.relative_path
                .len()
                .cmp(&right.relative_path.len())
                .then_with(|| left_index.cmp(right_index))
        });
        Ok(recursive_sources.into_iter().map(|(_, source)| source).collect())
    }

    fn bind_recursive_mounts(
        &self,
        recursive_sources: &[RecursiveBindSource],
        target: &Arc<Dentry>,
    ) -> SysResult<Vec<Arc<Mount>>> {
        let target_mount = target.mounted_root().ok_or(Errno::EINVAL)?.get_mount();
        let target_root = target_mount.root().ok_or(Errno::EINVAL)?;
        let mut created_mounts = Vec::new();

        let result = (|| -> SysResult<()> {
            for recursive_source in recursive_sources {
                let target = Self::lookup_relative_mountpoint_following_mounts(
                    target_root.clone(),
                    &recursive_source.relative_path,
                )?;
                Self::check_bind_target_type(recursive_source.is_dir, &target)?;

                let mount = target.bind_mount(&recursive_source.source, &recursive_source.source_root);
                if let Some(group) = recursive_source.mount.shared_group() {
                    mount.join_shared_group(&group);
                } else if let Some(group) = recursive_source.mount.master_group() {
                    mount.make_slave_of(&group);
                }

                created_mounts.push(mount);
            }

            Ok(())
        })();

        match result {
            Ok(()) => Ok(created_mounts),
            Err(err) => {
                Self::rollback_mounts(&created_mounts);
                Err(err)
            }
        }
    }
    /// Find peer mountpoints that should receive the same mount event.
    ///
    /// The input `mountpoint` belongs to `parent_mount`'s visible tree. It is first mapped back to
    /// the source tree, then mapped into each peer by using that peer's `source_root`.
    fn propagation_targets(
        &self,
        parent_mount: &Arc<Mount>,
        mountpoint: &Arc<Dentry>,
    ) -> SysResult<Vec<PropagationTarget>> {
        let Some(group) = parent_mount.shared_group() else {
            return Ok(Vec::new());
        };
        let parent_source_root = parent_mount.source_root().ok_or(Errno::EINVAL)?;

        // Mount event broadcast step 1:
        // translate the event location from `parent_mount`'s visible tree back
        // to the original source tree, using `parent_mount.source_root`.
        let source_dentry = Self::source_dentry_from_mount(parent_mount, mountpoint)?;
        let mut targets = Vec::new();
        for (receiver_mounts, kind) in [
            (group.peers(), PropagationTargetKind::Peer),
            (group.slaves(), PropagationTargetKind::Slave),
        ] {
            for receiver in receiver_mounts {
                if kind == PropagationTargetKind::Peer && Arc::ptr_eq(&receiver, parent_mount) {
                    continue;
                }
                let Some(receiver_source_root) = receiver.source_root() else {
                    continue;
                };
                // Peers in the same propagation group may represent different bind roots.
                // Only peers rooted like the sender receive this mount event.
                if !Arc::ptr_eq(&receiver_source_root, &parent_source_root) {
                    continue;
                }
                // Mount event broadcast step 2:
                // find the same source-tree location under this receiver's source root,
                // then replay that relative path from the receiver's visible root.
                let Some(relative_path) = Self::relative_path_from_dentry(&receiver_source_root, &source_dentry) else {
                    continue;
                };
                let Some(receiver_root) = receiver.root() else {
                    continue;
                };
                let target = Self::lookup_relative_mountpoint(receiver_root, &relative_path)?;
                if Self::mount_chain_contains(&target, mountpoint) {
                    continue;
                }
                if targets
                    .iter()
                    .any(|existing: &PropagationTarget| Arc::ptr_eq(&existing.dentry, &target))
                {
                    continue;
                }
                targets.push(PropagationTarget { dentry: target, kind });
            }
        }
        Ok(targets)
    }

    /// Return `dentry`'s path relative to the visible root of `mount`.
    fn relative_path_from_mount(mount: &Arc<Mount>, dentry: &Arc<Dentry>) -> SysResult<Vec<String>> {
        let root = mount.root().ok_or(Errno::EINVAL)?;
        Self::relative_path_from_dentry(&root, dentry).ok_or(Errno::EINVAL)
    }

    /// Return `dentry`'s path relative to `root` if `dentry` is inside that tree.
    fn relative_path_from_dentry(root: &Arc<Dentry>, dentry: &Arc<Dentry>) -> Option<Vec<String>> {
        let mut current = Some(dentry.clone());
        let mut path = Vec::new();
        let mut visited = Vec::new();

        while let Some(dentry) = current {
            if Arc::ptr_eq(&dentry, root) {
                path.reverse();
                return Some(path);
            }
            if visited.iter().any(|visited| Arc::ptr_eq(visited, &dentry)) {
                return None;
            }
            visited.push(dentry.clone());
            path.push(dentry.name());
            current = dentry.get_parent();
        }

        None
    }
    /// Map `dentry` from `mount`'s visible tree back to the original source tree.
    ///
    /// Bind mounts create a new visible tree whose dentries mirror the source, but
    /// the visible dentry is not always the same object as the dentry in the source
    /// tree. For example:
    ///
    /// ```bash
    /// mount --bind /real /src
    /// mount --bind /src/a /dst
    /// ```
    ///
    /// During the second command, the visible source path is `/src/a`, but the
    /// source-tree dentry that must be remembered by the new bind mount is
    /// `/real/a`.
    ///
    /// To recover the source-tree dentry, first compute `dentry`'s path relative to
    /// `mount.root()`, then replay that relative path from `mount.source_root()`.
    /// The result is used as the `source_root` for nested bind mounts and as the
    /// anchor for mount propagation between shared peers.
    fn source_dentry_from_mount(mount: &Arc<Mount>, dentry: &Arc<Dentry>) -> SysResult<Arc<Dentry>> {
        let source_root = mount.source_root().ok_or(Errno::EINVAL)?;
        let relative_path = Self::relative_path_from_mount(mount, dentry)?;
        Self::lookup_relative_mountpoint(source_root, &relative_path)
    }

    /// Walk a relative path from `current` and return the target mountpoint dentry.
    fn lookup_relative_mountpoint(mut current: Arc<Dentry>, path: &[String]) -> SysResult<Arc<Dentry>> {
        for name in path {
            current = current.lookup(name)?;
        }
        Ok(current)
    }

    fn lookup_relative_mountpoint_following_mounts(
        mut current: Arc<Dentry>,
        path: &[String],
    ) -> SysResult<Arc<Dentry>> {
        for (index, name) in path.iter().enumerate() {
            current = current.lookup(name)?;
            if index + 1 < path.len() {
                current = current.get_mount_to();
            }
        }
        Ok(current)
    }

    /// Check whether following stacked mounts from `dentry` reaches `target`.
    fn mount_chain_contains(dentry: &Arc<Dentry>, target: &Arc<Dentry>) -> bool {
        let mut current = dentry.clone();
        let mut visited = Vec::new();
        loop {
            if Arc::ptr_eq(&current, target) {
                return true;
            }
            if visited.iter().any(|visited| Arc::ptr_eq(visited, &current)) {
                return false;
            }
            visited.push(current.clone());
            let Some(mounted_root) = current.mounted_root() else {
                return false;
            };
            current = mounted_root;
        }
    }
    fn resolve_top_mountpoint(dentry: Arc<Dentry>) -> SysResult<Arc<Dentry>> {
        let mounted_root = dentry.clone().get_mount_to();
        if Arc::ptr_eq(&mounted_root, &dentry) {
            return Ok(dentry);
        }
        mounted_root.get_mount().mountpoint().ok_or(Errno::EINVAL)
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

    fn make_slave(&self, root: &Arc<Dentry>, dir: &Arc<Dentry>, path: &str, recursive: bool) -> SysResult<()> {
        let dentry = self.resolve_mountpoint(root, dir, path)?;
        let mount = dentry.mounted_root().ok_or(Errno::EINVAL)?.get_mount();
        mount.make_slave();

        if recursive {
            let mounts = self.mounts.lock().clone();
            for slave_mount in mounts {
                if !slave_mount.is_descendant_of(&mount) {
                    continue;
                }
                slave_mount.make_slave();
            }
        }

        Ok(())
    }

    fn make_unbindable(&self, root: &Arc<Dentry>, dir: &Arc<Dentry>, path: &str, recursive: bool) -> SysResult<()> {
        let dentry = self.resolve_mountpoint(root, dir, path)?;
        let mount = dentry.mounted_root().ok_or(Errno::EINVAL)?.get_mount();
        mount.make_unbindable();

        if recursive {
            let mounts = self.mounts.lock().clone();
            for unbindable_mount in mounts {
                if !unbindable_mount.is_descendant_of(&mount) {
                    continue;
                }
                unbindable_mount.make_unbindable();
            }
        }

        Ok(())
    }

    fn unmount(&self, root: &Arc<Dentry>, dir: &Arc<Dentry>, path: &str) -> SysResult<()> {
        let dentry = self.resolve_mountpoint(root, dir, path)?;
        if dentry.mounted_root().is_none() {
            return Err(Errno::EINVAL);
        }
        let mounted_root = dentry.mounted_root().ok_or(Errno::EINVAL)?;
        let mounted_mount = mounted_root.get_mount();
        let parent_mount = mounted_mount.parent().ok_or(Errno::EINVAL)?;
        let mountpoint = mounted_mount.mountpoint().ok_or(Errno::EINVAL)?;
        let mut mountpoints = Vec::new();
        mountpoints.push(dentry);
        let propagated_mountpoints = self.propagation_targets(&parent_mount, &mountpoint)?;
        // Propagation gives us the peer location; unmount the top mount at that location.
        for propagated_mountpoint in propagated_mountpoints {
            mountpoints.push(Self::resolve_top_mountpoint(propagated_mountpoint.dentry)?);
        }

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
        if mounts.iter().any(|mounted| {
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
            mount.clear_source_root();
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

pub fn bind_mount(
    root: &Arc<Dentry>,
    dir: &Arc<Dentry>,
    source: &str,
    target: &str,
    recursive: bool,
) -> Result<(), Errno> {
    vfs().bind_mount(root, dir, source, target, recursive)
}

pub fn move_mount(root: &Arc<Dentry>, dir: &Arc<Dentry>, source: &str, target: &str) -> Result<(), Errno> {
    vfs().move_mount(root, dir, source, target)
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

pub fn make_slave(root: &Arc<Dentry>, dir: &Arc<Dentry>, path: &str, recursive: bool) -> Result<(), Errno> {
    vfs().make_slave(root, dir, path, recursive)
}

pub fn make_unbindable(root: &Arc<Dentry>, dir: &Arc<Dentry>, path: &str, recursive: bool) -> Result<(), Errno> {
    vfs().make_unbindable(root, dir, path, recursive)
}

pub fn unmount_all() -> SysResult<()> {
    let vfs = vfs();
    let _ = vfs.cache.sync();
    let mounts = vfs.mounts.lock().clone();
    for mount in mounts {
        mount.make_private();
        mount.clear_source_root();
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
