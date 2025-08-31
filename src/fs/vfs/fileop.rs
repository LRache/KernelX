use alloc::sync::Arc;

use crate::fs::inode::manager::InodeWrapper;
use crate::kernel::errno::Errno;
use crate::fs::file::{File, FileFlags};
use crate::fs::vfs::VFS;

pub fn open(path: &str, flags: FileFlags) -> Result<File, Errno> {
    let inode_index = VFS.lookup_inode(path, 0, 0)?;
    let inode = VFS.open_inode(inode_index.fsno, inode_index.ino)?;

    Ok(File::new(inode, flags))
}

pub fn openat(dir: &Arc<InodeWrapper>, path: &str, flags: FileFlags) -> Result<File, Errno> {
    let fsno = dir.get_fsno();
    let ino = dir.get_ino();
    
    let inode_index = VFS.lookup_inode(path, fsno, ino)?;
    let inode = VFS.open_inode(inode_index.fsno, inode_index.ino)?;

    Ok(File::new(inode, flags))
}
