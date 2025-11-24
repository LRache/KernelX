mod dentry;
mod fileop;
mod fsop;
mod init;
mod superblock_table;
mod vfs;

use superblock_table::SuperBlockTable;

pub use dentry::Dentry;
pub use fileop::*;
pub use fsop::*;
pub use init::init;

use crate::klib::InitedCell;
use vfs::VirtualFileSystem;

static VFS: InitedCell<VirtualFileSystem> = InitedCell::uninit();

pub(super) fn vfs() -> &'static VirtualFileSystem {
    &VFS
}
