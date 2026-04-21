use alloc::sync::Arc;
use alloc::vec::Vec;

use crate::driver::BlockDriverOps;
use crate::fs::filesystem::{FileSystemOps, MountOptions, SuperBlockOps};
use crate::fs::vfs::vfs::VirtualFileSystem;
use crate::kernel::errno::{Errno, SysResult};
use crate::kernel::uapi::Statfs;

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
        dir: &Arc<Dentry>,
        path: &str,
        fstype_name: &str,
        device: Option<Arc<dyn BlockDriverOps>>,
        options: MountOptions,
    ) -> SysResult<()> {
        let dentry = self.lookup_dentry(dir, path)?;

        let fstype = self.fstype_map.get(fstype_name).ok_or(Errno::ENOENT)?;

        let (sno, root_ino) = {
            let mut superblock_table = self.superblock_table.lock();
            let sno = superblock_table.mount(*fstype, device, options)?;
            (sno, superblock_table.get(sno).unwrap().get_root_ino())
        };

        let root_inode = self.load_inode(sno, root_ino)?;

        dentry.mount(&root_inode, sno);

        self.mountpoint.lock().push(dentry);

        Ok(())
    }

    fn resolve_mountpoint(&self, dir: &Arc<Dentry>, path: &str) -> SysResult<Arc<Dentry>> {
        match self.lookup_parent_dentry(dir, path)? {
            Some((parent, name)) => parent.lookup(name),
            None => Ok(self.get_root().clone()),
        }
    }

    fn unmount(&self, dir: &Arc<Dentry>, path: &str) -> SysResult<()> {
        fn is_descendant_mount(parent: &str, child: &str) -> bool {
            if child == parent {
                return false;
            }
            if parent == "/" {
                return child.starts_with('/');
            }
            child.strip_prefix(parent).is_some_and(|suffix| suffix.starts_with('/'))
        }

        let dentry = self.resolve_mountpoint(dir, path)?;
        let mounted_root = dentry.mounted_root().ok_or(Errno::EINVAL)?;
        let mounted_sno = mounted_root.sno();
        let mount_path = dentry.get_path();

        let mountpoints = self.mountpoint.lock().clone();
        if mountpoints.iter().any(|mountpoint| {
            !Arc::ptr_eq(mountpoint, &dentry) && is_descendant_mount(&mount_path, &mountpoint.get_path())
        }) {
            crate::kinfo!("Unmount failed: mountpoint {} has descendant mountpoints", mount_path);
            return Err(Errno::EBUSY);
        }

        if self.cache.superblock_busy(mounted_sno) {
            crate::kinfo!("Unmount failed: superblock {} is busy", mounted_sno);
            return Err(Errno::EBUSY);
        }

        dentry.unmount().ok_or(Errno::EINVAL)?;
        self.mountpoint
            .lock()
            .retain(|mountpoint| !Arc::ptr_eq(mountpoint, &dentry));
        self.cache.remove_superblock(mounted_sno);
        self.superblock_table.lock().unmount(mounted_sno)?;

        Ok(())
    }

    fn sync_all(&self) -> SysResult<()> {
        self.cache.sync()?;
        self.superblock_table.lock().sync_all()?;
        Ok(())
    }

    pub(super) fn is_superblock_readonly(&self, sno: u32) -> SysResult<bool> {
        Ok(self.get_superblock(sno)?.is_readonly())
    }

    pub fn mountpoint_list(&self) -> Vec<Arc<Dentry>> {
        self.mountpoint.lock().clone()
    }
}

pub fn mount(
    dir: &Arc<Dentry>,
    path: &str,
    fstype_name: &str,
    device: Option<Arc<dyn BlockDriverOps>>,
    options: MountOptions,
) -> Result<(), Errno> {
    vfs().mount(dir, path, fstype_name, device, options)
}

pub fn unmount(dir: &Arc<Dentry>, path: &str) -> Result<(), Errno> {
    vfs().unmount(dir, path)
}

pub fn get_root_dentry() -> &'static Arc<Dentry> {
    vfs().get_root()
}

pub fn statfs(sno: u32) -> SysResult<Statfs> {
    let superblock = vfs().get_superblock(sno).unwrap();

    superblock.statfs()
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
