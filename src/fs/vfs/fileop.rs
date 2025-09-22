use alloc::sync::Arc;
use alloc::string::String;

use crate::fs::vfs::dentry::Dentry;
use crate::fs::file::{File, FileFlags};
use crate::kernel::errno::{Errno, SysResult};

use super::vfs;

pub fn open_dentry(path: &str, _flags: FileFlags) -> Result<Arc<Dentry>, Errno> {
    vfs().lookup_dentry(vfs().get_root(), path)
}

pub fn open_parent_dentry(path: &str) -> SysResult<(Arc<Dentry>, String)> {
    vfs().lookup_parent_dentry(vfs().get_root(), path)
}

pub fn open_file(path: &str, flags: FileFlags) -> Result<File, Errno> {
    let dentry = vfs().lookup_dentry(vfs().get_root(), path)?;
    Ok(File::new(&dentry, flags))
}

pub fn openat_dentry(dir: &Arc<Dentry>, path: &str, _flags: FileFlags) -> SysResult<Arc<Dentry>> {
    vfs().lookup_dentry(dir, path)
}

pub fn openat_parent_dentry(dir: &Arc<Dentry>, path: &str) -> SysResult<(Arc<Dentry>, String)> {
    vfs().lookup_parent_dentry(dir, path)
}

pub fn openat_file(dir: &Arc<Dentry>, path: &str, flags: FileFlags) -> SysResult<File> {
    let dentry = vfs().lookup_dentry(dir, path)?;
    Ok(File::new(&dentry, flags))
}

pub fn create_file(dir: &Arc<Dentry>, name: &str, flags: FileFlags) -> SysResult<File> {
    // let dentry = dir.mkdir(name)?;
    // Ok(File::new(&dentry, flags))
    Err(Errno::EOPNOTSUPP)
}
