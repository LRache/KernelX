mod vfs;
mod rootfs;
mod fileop;
mod fsop;
mod dentry;
mod superblock_table;

use superblock_table::SuperBlockTable;

pub mod stdout;
pub use dentry::Dentry;
pub use fileop::*;
pub use fsop::*;

use vfs::VirtualFileSystem;

static VFS: VirtualFileSystem = VirtualFileSystem::new();

pub(super) fn vfs() -> &'static VirtualFileSystem {
    &VFS
}

pub fn init() {
    VFS.init();
}

pub fn fini() {
    // VFS.fini();
}
