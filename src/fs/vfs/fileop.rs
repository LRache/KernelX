use alloc::sync::Arc;
use alloc::string::String;

use crate::fs::inode::{InodeOps, Mode};
use crate::fs::vfs::dentry::Dentry;
use crate::fs::file::{File, FileFlags};
use crate::kernel::errno::{Errno, SysResult};

use super::vfs;

fn new_file(dentry: &Arc<Dentry>, flags: FileFlags) -> File {
    if dentry.get_inode().mode().contains(Mode::S_IFIFO) {
        unimplemented!() // return Pipe::new_fifo(...);
    }
    File::new(dentry, flags)
}

pub fn load_dentry(path: &str) -> Result<Arc<Dentry>, Errno> {
    vfs().lookup_dentry(vfs().get_root(), path)
}

pub fn load_parent_dentry(path: &str) -> SysResult<(Arc<Dentry>, String)> {
    vfs().lookup_parent_dentry(vfs().get_root(), path)
}

pub fn open_file(path: &str, flags: FileFlags) -> Result<File, Errno> {
    let dentry = vfs().lookup_dentry(vfs().get_root(), path)?;
    Ok(new_file(&dentry, flags))
}

pub fn load_dentry_at(dir: &Arc<Dentry>, path: &str) -> SysResult<Arc<Dentry>> {
    vfs().lookup_dentry(dir, path)
}

pub fn load_parent_dentry_at(dir: &Arc<Dentry>, path: &str) -> SysResult<(Arc<Dentry>, String)> {
    vfs().lookup_parent_dentry(dir, path)
}

pub fn openat_file(dir: &Arc<Dentry>, path: &str, flags: FileFlags) -> SysResult<File> {
    let dentry = vfs().lookup_dentry(dir, path)?;
    Ok(new_file(&dentry, flags))
}

pub fn load_inode(sno: u32, ino: u32) -> SysResult<Arc<dyn InodeOps>> {
    vfs().load_inode(sno, ino)
}

pub fn create_temp(dentry: &Arc<Dentry>, flags: FileFlags, mode: Mode) -> SysResult<File> {
    let superblock = vfs().superblock_table.lock().get(dentry.sno()).ok_or(Errno::ENOENT)?;
    let inode = superblock.create_temp(mode)?;

    let inode: Arc<dyn InodeOps> = Arc::from(inode);
    let dentry = Arc::new(Dentry::new("", dentry, &inode));

    Ok(File::new_inode(inode, dentry, flags))
}
