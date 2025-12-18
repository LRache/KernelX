use alloc::sync::Arc;

use crate::fs::devfs;
use crate::fs::ext4::Ext4FileSystem;
use crate::fs::tmpfs;
use crate::fs::rootfs::RootFileSystem;
use crate::fs::vfs::VFS;
use crate::fs::vfs::vfs::VirtualFileSystem;
use crate::fs::Dentry;

#[unsafe(link_section = ".text.init")]
pub fn init() {
    let mut vfs = VirtualFileSystem::new();
    vfs.register_filesystem("devfs", &devfs::FileSystem);
    vfs.register_filesystem("ext4", &Ext4FileSystem);
    vfs.register_filesystem("tmpfs", &tmpfs::FileSystem);

    vfs.superblock_table.lock().mount(&RootFileSystem, None).unwrap();
    vfs.root.init(Arc::new(Dentry::root(&vfs.load_inode(0, 0).unwrap(), 0)));

    VFS.init(vfs);
}