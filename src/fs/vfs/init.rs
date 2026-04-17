use alloc::sync::Arc;
use core::time::Duration;

use crate::fs::ext4::Ext4FileSystem;
use crate::fs::rootfs::RootFileSystem;
use crate::fs::vfs::VFS;
use crate::fs::vfs::vfs::VirtualFileSystem;
use crate::fs::{Dentry, devfs, procfs, tmpfs};
use crate::kernel::kthread;
use crate::kernel::scheduler::current;

const INODE_CACHE_REAPER_INTERVAL: Duration = Duration::from_secs(1);

fn inode_cache_reaper() {
    loop {
        current::sleep(INODE_CACHE_REAPER_INTERVAL);
        super::vfs().cache.prune_unused();
    }
}

pub fn spawn_inode_cache_reaper() {
    kthread::spawn(inode_cache_reaper);
}

#[unsafe(link_section = ".text.init")]
pub fn init() {
    let mut vfs = VirtualFileSystem::new();
    vfs.register_filesystem("devfs", &devfs::FileSystem);
    vfs.register_filesystem("ext4", &Ext4FileSystem);
    vfs.register_filesystem("tmpfs", &tmpfs::FileSystem);
    vfs.register_filesystem("procfs", &procfs::FileSystem);

    vfs.superblock_table.lock().mount(&RootFileSystem, None).unwrap();
    vfs.root.init(Arc::new(Dentry::root(&vfs.load_inode(0, 0).unwrap(), 0)));

    VFS.init(vfs);
}
