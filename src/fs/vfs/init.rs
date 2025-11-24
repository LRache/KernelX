use alloc::sync::Arc;

use crate::fs::Dentry;
use crate::fs::devfs::DevFileSystem;
use crate::fs::ext4::Ext4FileSystem;
use crate::fs::rootfs::RootFileSystem;
use crate::fs::tmpfs;
use crate::fs::vfs::VFS;
use crate::fs::vfs::vfs::VirtualFileSystem;

#[unsafe(link_section = ".text.init")]
pub fn init() {
    let mut vfs = VirtualFileSystem::new();
    vfs.register_filesystem("devfs", &DevFileSystem);
    vfs.register_filesystem("ext4", &Ext4FileSystem);
    vfs.register_filesystem("tmpfs", &tmpfs::FileSystem);

    vfs.superblock_table
        .lock()
        .alloc(&RootFileSystem, None)
        .unwrap();
    vfs.root
        .init(Arc::new(Dentry::root(&vfs.load_inode(0, 0).unwrap())));

    VFS.init(vfs);
}
