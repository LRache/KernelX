mod dentry;
mod fileop;
mod fsop;
mod init;
mod mount;
mod path;
mod superblock_table;
mod vfs;

use superblock_table::SuperBlockTable;

pub use dentry::Dentry;
pub use fileop::*;
pub use fsop::*;
pub use init::{init, spawn_inode_cache_reaper};
pub use mount::*;

use crate::klib::InitedCell;
pub(super) use path::split_path;
pub use vfs::LookupFlags;
use vfs::VirtualFileSystem;

static VFS: InitedCell<VirtualFileSystem> = InitedCell::uninit();

pub(super) fn vfs() -> &'static VirtualFileSystem {
    &VFS
}
