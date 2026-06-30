use alloc::sync::Arc;
use core::time::Duration;

use crate::fs::filesystem::{FileSystemOps, SuperBlockOps};
use crate::fs::inode::Index;
use crate::fs::vfs::vfs::VirtualFileSystem;
use crate::kernel::config;
use crate::kernel::errno::{Errno, SysResult};
use crate::kernel::scheduler::current;
use crate::kernel::uapi::{Statfs, StatfsFlags};

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

    fn sync_all(&self) -> SysResult<()> {
        self.cache.sync()?;
        self.superblock_table.lock().sync_all()?;
        Ok(())
    }

    pub(super) fn is_superblock_readonly(&self, sno: u32) -> SysResult<bool> {
        self.superblock_table.lock().is_readonly(sno)
    }
}

pub fn get_fstype(fstype_name: &str) -> Option<&'static dyn FileSystemOps> {
    vfs().fstype_map.get(fstype_name).cloned()
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

pub fn evict_inode(sno: u32, ino: u32) {
    vfs().cache.remove(&Index { sno, ino });
}

pub fn find_cached_inode(sno: u32, ino: u32) -> Option<Arc<dyn crate::fs::InodeOps>> {
    vfs().cache.find(&Index { sno, ino })
}
