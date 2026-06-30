use alloc::sync::Arc;

use crate::fs::exfat::FileSystem as ExfatFileSystem;
use crate::fs::ext4::Ext4FileSystem;
use crate::fs::filesystem::MountOptions;
use crate::fs::rootfs::RootFileSystem;
use crate::fs::vfat::FileSystem as VfatFileSystem;
use crate::fs::vfs::VFS;
use crate::fs::vfs::vfs::VirtualFileSystem;
use crate::fs::{Dentry, devfs, ext4_native, procfs, tmpfs};

#[unsafe(link_section = ".text.init")]
pub fn init() {
    let mut vfs = VirtualFileSystem::new();
    vfs.register_filesystem("devfs", &devfs::FileSystem);
    vfs.register_filesystem("ext2", &Ext4FileSystem);
    vfs.register_filesystem("ext3", &Ext4FileSystem);
    vfs.register_filesystem("exfat", &ExfatFileSystem);
    vfs.register_filesystem("vfat", &VfatFileSystem);
    vfs.register_filesystem("tmpfs", &tmpfs::FileSystem);
    vfs.register_filesystem("procfs", &procfs::FileSystem);
    vfs.register_filesystem("ext4", &ext4_native::FileSystem);

    vfs.superblock_table
        .lock()
        .mount(&RootFileSystem, None, MountOptions::default())
        .unwrap();
    vfs.root.init(Arc::new(Dentry::root(&vfs.load_inode(0, 0).unwrap(), 0)));

    VFS.init(vfs);
}
