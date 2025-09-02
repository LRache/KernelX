use alloc::sync::Arc;

use crate::fs::vfs::dentry::Dentry;
use crate::fs::LockedInode;
use crate::kernel::errno::Errno;
use crate::fs::file::{File, FileFlags};
use crate::fs::vfs::VFS;

pub fn open(path: &str, flags: FileFlags) -> Result<File, Errno> {
    let inode_index = VFS.lookup_inode(path, 0, 0)?;
    let inode = VFS.open_inode(inode_index.sno, inode_index.ino)?;

    Ok(File::new(inode, flags))
}

pub fn openat(dir: &Arc<Dentry>, path: &str, flags: FileFlags) -> Result<File, Errno> {
    let sno = dir.get_sno();
    let ino = dir.get_ino();
    
    let inode_index = VFS.lookup_inode(path, sno, ino)?;
    let inode = VFS.open_inode(inode_index.sno, inode_index.ino)?;

    Ok(File::new(inode, flags))
}
