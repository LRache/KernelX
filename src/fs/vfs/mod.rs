mod vfs;
mod fileop;
mod fsop;
mod dentry;
mod superblock_table;
mod init;

use superblock_table::SuperBlockTable;

pub use dentry::Dentry;
pub use fileop::*;
pub use fsop::*;
pub use init::init;

use vfs::VirtualFileSystem;
use crate::klib::InitedCell;

static VFS: InitedCell<VirtualFileSystem> = InitedCell::uninit();

pub(super) fn vfs() -> &'static VirtualFileSystem {
    &VFS
}
