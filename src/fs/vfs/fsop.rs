use alloc::sync::Arc;
use alloc::vec::Vec;
use core::time::Duration;

use crate::driver::BlockDriverOps;
use crate::fs::filesystem::{FileSystemOps, SuperBlockOps};
use crate::fs::inode::Index;
use crate::fs::vfs::vfs::VirtualFileSystem;
use crate::kernel::config;
use crate::kernel::errno::{Errno, SysResult};
use crate::kernel::scheduler::current;
use crate::kernel::uapi::Statfs;

use super::{Dentry, vfs};

pub fn inode_cache_reclaimer() {
    loop {
        current::sleep(Duration::from_millis(config::INODE_CACHE_RECLAIM_INTERVAL_MS));
        vfs().cache.prune_unused();
    }
}

impl VirtualFileSystem {
    pub(super) fn register_filesystem(&mut self, name: &'static str, fs: &'static dyn FileSystemOps) {
        self.fstype_map.insert(name, fs);
    }

    fn get_superblock(&self, sno: u32) -> SysResult<Arc<dyn SuperBlockOps>> {
        let superblock_table = self.superblock_table.lock();
        let superblock = superblock_table.get(sno).ok_or(Errno::EINVAL)?;

        Ok(superblock)
    }

    fn mount(&self, path: &str, fstype_name: &str, device: Option<Arc<dyn BlockDriverOps>>) -> SysResult<()> {
        let dentry = self.lookup_dentry(self.get_root(), path)?;

        let fstype = self.fstype_map.get(fstype_name).ok_or(Errno::ENOENT)?;

        let (sno, root_ino) = {
            let mut superblock_table = self.superblock_table.lock();
            let sno = superblock_table.mount(*fstype, device)?;
            (sno, superblock_table.get(sno).unwrap().get_root_ino())
        };

        let root_inode = self.load_inode(sno, root_ino)?;

        dentry.mount(&root_inode, sno);

        self.mountpoint.lock().push(dentry);

        Ok(())
    }

    fn sync_all(&self) -> SysResult<()> {
        self.cache.sync()?;
        self.superblock_table.lock().sync_all()?;
        Ok(())
    }

    pub fn mountpoint_list(&self) -> Vec<Arc<Dentry>> {
        self.mountpoint.lock().clone()
    }
}

pub fn mount(path: &str, fstype_name: &str, device: Option<Arc<dyn BlockDriverOps>>) -> Result<(), Errno> {
    vfs().mount(path, fstype_name, device)
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
    vfs().cache.clear();
    {
        let mut superblock_table = vfs().superblock_table.lock();
        superblock_table.unmount_all()?;
    }

    Ok(())
}

pub fn evict_inode(sno: u32, ino: u32) {
    vfs().cache.remove(&Index { sno, ino });
}

pub fn find_cached_inode(sno: u32, ino: u32) -> Option<Arc<dyn crate::fs::InodeOps>> {
    vfs().cache.find(&Index { sno, ino })
}
