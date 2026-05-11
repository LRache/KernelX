use alloc::sync::Arc;
use alloc::vec::Vec;

use crate::driver::BlockDriverOps;
use crate::fs::filesystem::{FileSystemOps, MountOptions, SuperBlockOps};
use crate::fs::vfs::vfs::VirtualFileSystem;
use crate::kernel::errno::{Errno, SysResult};
use crate::kernel::uapi::{Statfs, StatfsFlags};

use super::{Dentry, vfs};

impl VirtualFileSystem {
    pub(super) fn register_filesystem(&mut self, name: &'static str, fs: &'static dyn FileSystemOps) {
        self.fstype_map.insert(name, fs);
    }

    fn get_superblock(&self, sno: u32) -> SysResult<Arc<dyn SuperBlockOps>> {
        let superblock_table = self.superblock_table.lock();
        let superblock = superblock_table.get(sno).ok_or(Errno::EINVAL)?;

        Ok(superblock)
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
        let dentry = self.lookup_dentry(root, dir, path)?;

        let (sno, root_ino) = {
            let mut superblock_table = self.superblock_table.lock();
            let sno = superblock_table.mount(fstype, device, options)?;
            (sno, superblock_table.get(sno).unwrap().get_root_ino())
        };

        let root_inode = self.load_inode(sno, root_ino)?;

        dentry.mount(&root_inode, sno);

        self.mountpoint.lock().push(dentry);

        Ok(())
    }

    fn bind_mount(&self, root: &Arc<Dentry>, dir: &Arc<Dentry>, source: &str, target: &str) -> SysResult<()> {
        let source = self.lookup_dentry(root, dir, source)?;
        let target = self.resolve_mountpoint(root, dir, target)?;

        target.bind_mount(&source);
        self.mountpoint.lock().push(target);

        Ok(())
    }

    fn resolve_mountpoint(&self, root: &Arc<Dentry>, dir: &Arc<Dentry>, path: &str) -> SysResult<Arc<Dentry>> {
        match self.lookup_parent_dentry(root, dir, path)? {
            Some((parent, name)) => parent.lookup(name.as_ref()),
            None => Ok(root.clone()),
        }
    }

    fn remount(&self, root: &Arc<Dentry>, dir: &Arc<Dentry>, path: &str, options: MountOptions) -> SysResult<()> {
        let dentry = self.resolve_mountpoint(root, dir, path)?;
        let mounted_root = dentry.mounted_root().ok_or(Errno::EINVAL)?;
        self.superblock_table.lock().remount(mounted_root.sno(), options)
    }

    fn unmount(&self, root: &Arc<Dentry>, dir: &Arc<Dentry>, path: &str) -> SysResult<()> {
        fn is_descendant_mount(parent: &str, child: &str) -> bool {
            if child == parent {
                return false;
            }
            if parent == "/" {
                return child.starts_with('/');
            }
            child.strip_prefix(parent).is_some_and(|suffix| suffix.starts_with('/'))
        }

        let dentry = self.resolve_mountpoint(root, dir, path)?;
        let mounted_root = dentry.mounted_root().ok_or(Errno::EINVAL)?;
        let is_bind_mount = dentry.is_bind_mount();
        let mounted_sno = mounted_root.sno();
        let mount_path = dentry.get_path();

        let mountpoints = self.mountpoint.lock().clone();
        if mountpoints.iter().any(|mountpoint| {
            !Arc::ptr_eq(mountpoint, &dentry) && is_descendant_mount(&mount_path, &mountpoint.get_path())
        }) {
            crate::kinfo!("Unmount failed: mountpoint {} has descendant mountpoints", mount_path);
            return Err(Errno::EBUSY);
        }

        if !is_bind_mount && self.cache.superblock_busy(mounted_sno) {
            crate::kinfo!(
                "Unmount failed: superblock {} is busy, type={}",
                mounted_sno,
                self.superblock_table.lock().get(mounted_sno).unwrap().type_name()
            );
            return Err(Errno::EBUSY);
        }

        dentry.unmount().ok_or(Errno::EINVAL)?;
        self.mountpoint
            .lock()
            .retain(|mountpoint| !Arc::ptr_eq(mountpoint, &dentry));
        if !is_bind_mount {
            self.cache.remove_superblock(mounted_sno);
            self.superblock_table.lock().unmount(mounted_sno)?;
        }

        Ok(())
    }

    fn sync_all(&self) -> SysResult<()> {
        self.cache.sync()?;
        self.superblock_table.lock().sync_all()?;
        Ok(())
    }

    pub(super) fn is_superblock_readonly(&self, sno: u32) -> SysResult<bool> {
        self.superblock_table.lock().is_readonly(sno)
    }

    pub fn mountpoint_list(&self) -> Vec<Arc<Dentry>> {
        self.mountpoint.lock().clone()
    }
}

pub fn get_fstype(fstype_name: &str) -> Option<&'static dyn FileSystemOps> {
    vfs().fstype_map.get(fstype_name).cloned()
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

pub fn get_root_dentry() -> &'static Arc<Dentry> {
    vfs().get_root()
}

pub fn statfs(sno: u32) -> SysResult<Statfs> {
    let vfs = vfs();
    let readonly = vfs.is_superblock_readonly(sno)?;
    let mut statfs = vfs.get_superblock(sno)?.statfs()?;

    if readonly {
        statfs.f_flag |= StatfsFlags::ST_RDONLY.bits();
    } else {
        statfs.f_flag &= !StatfsFlags::ST_RDONLY.bits();
    }
    Ok(statfs)
}

pub fn sync_all() -> Result<(), Errno> {
    vfs().sync_all()
}

pub fn unmount_all() -> SysResult<()> {
    let vfs = vfs();
    let _ = vfs.cache.sync();
    vfs.cache.clear();
    let mut superblock_table = vfs.superblock_table.lock();
    superblock_table.unmount_all()?;

    Ok(())
}
