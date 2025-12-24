use alloc::sync::Arc;

use crate::fs::inode::Mode;
use crate::fs::perm::Perm;
use crate::fs::vfs::dentry::{self, Dentry};
use crate::fs::file::{File, FileFlags, FileOps};
use crate::kernel::errno::{Errno, SysResult};

use super::vfs;

fn new_file(dentry: Arc<Dentry>, flags: FileFlags, perm: &Perm) -> SysResult<Arc<dyn FileOps>> {
    let inode = dentry.get_inode();
    let mode = inode.mode()?;

    let (uid, gid) = inode.owner()?;
    if !mode.check_perm(perm, uid, gid) {
        return Err(Errno::EACCES);
    }

    if mode.contains(Mode::S_IFIFO) {
        unimplemented!() // TODO: return Pipe::new_fifo(...);
    }

    Ok(inode.wrap_file(Some(dentry), flags))
}

pub fn load_dentry(path: &str) -> SysResult<Arc<Dentry>> {
    vfs().lookup_dentry(vfs().get_root(), path)
}

pub fn load_parent_dentry<'a>(path: &'a str) -> SysResult<Option<(Arc<Dentry>, &'a str)>> {
    vfs().lookup_parent_dentry(vfs().get_root(), path)
}

pub fn open_file(path: &str, flags: FileFlags, perm: &Perm) -> SysResult<Arc<dyn FileOps>> {
    let dentry = vfs().lookup_dentry(vfs().get_root(), path)?;
    new_file(dentry, flags, perm)
}

pub fn load_dentry_at(dir: &Arc<Dentry>, path: &str) -> SysResult<Arc<Dentry>> {
    vfs().lookup_dentry(dir, path)
}

pub fn load_dentry_at_nofollow(dir: &Arc<Dentry>, path: &str) -> SysResult<Arc<Dentry>> {
    let dentry = vfs().lookup_dentry_nofollow(dir, path)?;
    Ok(dentry)
}

pub fn load_parent_dentry_at<'a>(dir: &Arc<Dentry>, path: &'a str) -> SysResult<Option<(Arc<Dentry>, &'a str)>> {
    vfs().lookup_parent_dentry(dir, path)
}

pub fn openat_file(dir: &Arc<Dentry>, path: &str, flags: FileFlags, perm: &Perm) -> SysResult<Arc<dyn FileOps>> {
    let dentry = vfs().lookup_dentry(dir, path)?;
    new_file(dentry, flags, perm)
}

pub fn create_file(dir: &Arc<Dentry>, name: &str, flags: FileFlags, mode: Mode) -> SysResult<Arc<dyn FileOps>> {
   let inode = dir.create(name, mode)?;
   let dentry = Arc::new(dentry::Dentry::new(name, dir, &inode, dir.sno()));
   Ok(inode.wrap_file(Some(dentry), flags))
}

pub fn create_temp(dentry: &Arc<Dentry>, flags: FileFlags, mode: Mode) -> SysResult<Arc<dyn FileOps>> {
    let superblock = vfs().superblock_table.lock().get(dentry.sno()).ok_or(Errno::ENOENT)?;
    let inode = superblock.create_temp(mode)?;
    let dentry = Arc::new(Dentry::new("", dentry, &inode, dentry.sno()));

    Ok(Arc::new(File::new(inode, dentry, flags)))
}
