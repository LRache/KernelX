use alloc::borrow::Cow;
use alloc::sync::Arc;

use crate::fs::file::{FileFlags, FileOps, RandomAccessFile};
use crate::fs::inode::{FileType, Mode, Owner};
use crate::fs::perm::Perm;
use crate::fs::vfs::dentry::{self, Dentry};
use crate::kernel::errno::{Errno, SysResult};

use super::{LookupFlags, vfs};

fn new_file(dentry: Arc<Dentry>, flags: FileFlags, perm: &Perm) -> SysResult<Arc<dyn FileOps>> {
    let inode = dentry.get_inode();
    let mode = inode.mode()?;

    if flags.writable && (mode & Mode::S_IFMT) == Mode::S_IFDIR {
        return Err(Errno::EISDIR);
    }

    if flags.writable && dentry.is_superblock_readonly()? {
        return Err(Errno::EROFS);
    }

    let (uid, gid) = inode.owner()?;
    if !mode.check_perm(perm, uid, gid) {
        return Err(Errno::EACCES);
    }

    Ok(inode.wrap_file(Some(dentry), flags))
}

pub fn load_dentry(path: &str) -> SysResult<Arc<Dentry>> {
    vfs().lookup_dentry(vfs().get_root(), vfs().get_root(), path)
}

pub fn open_file(path: &str, flags: FileFlags, perm: &Perm) -> SysResult<Arc<dyn FileOps>> {
    let dentry = vfs().lookup_dentry(vfs().get_root(), vfs().get_root(), path)?;
    new_file(dentry, flags, perm)
}

pub fn load_dentry_at(root: &Arc<Dentry>, dir: &Arc<Dentry>, path: &str) -> SysResult<Arc<Dentry>> {
    vfs().lookup_dentry(root, dir, path)
}

pub fn load_dentry_at_with_flags(
    root: &Arc<Dentry>,
    dir: &Arc<Dentry>,
    path: &str,
    flags: LookupFlags,
) -> SysResult<Arc<Dentry>> {
    vfs().lookup_dentry_with_flags(root, dir, path, flags)
}

pub fn load_dentry_at_with_perm(
    root: &Arc<Dentry>,
    dir: &Arc<Dentry>,
    path: &str,
    perm: &Perm,
) -> SysResult<Arc<Dentry>> {
    vfs().lookup_dentry_with_perm(root, dir, path, perm)
}

pub fn load_dentry_at_nofollow(root: &Arc<Dentry>, dir: &Arc<Dentry>, path: &str) -> SysResult<Arc<Dentry>> {
    let dentry = vfs().lookup_dentry_nofollow(root, dir, path)?;
    Ok(dentry)
}

pub fn load_dentry_at_nofollow_with_perm(
    root: &Arc<Dentry>,
    dir: &Arc<Dentry>,
    path: &str,
    perm: &Perm,
) -> SysResult<Arc<Dentry>> {
    let dentry = vfs().lookup_dentry_nofollow_with_perm(root, dir, path, perm)?;
    Ok(dentry)
}

pub fn load_dentry_at_nofollow_with_perm_and_flags(
    root: &Arc<Dentry>,
    dir: &Arc<Dentry>,
    path: &str,
    perm: &Perm,
    flags: LookupFlags,
) -> SysResult<Arc<Dentry>> {
    let dentry = vfs().lookup_dentry_nofollow_with_perm_and_flags(root, dir, path, perm, flags)?;
    Ok(dentry)
}

pub fn load_parent_dentry_at<'a>(
    root: &Arc<Dentry>,
    dir: &Arc<Dentry>,
    path: &'a str,
) -> SysResult<Option<(Arc<Dentry>, Cow<'a, str>)>> {
    vfs().lookup_parent_dentry(root, dir, path)
}

pub fn load_parent_dentry_at_with_flags<'a>(
    root: &Arc<Dentry>,
    dir: &Arc<Dentry>,
    path: &'a str,
    flags: LookupFlags,
) -> SysResult<Option<(Arc<Dentry>, Cow<'a, str>)>> {
    vfs().lookup_parent_dentry_with_flags(root, dir, path, flags)
}

pub fn openat_file(
    root: &Arc<Dentry>,
    dir: &Arc<Dentry>,
    path: &str,
    flags: FileFlags,
    perm: &Perm,
) -> SysResult<Arc<dyn FileOps>> {
    let dentry = vfs().lookup_dentry(root, dir, path)?;
    new_file(dentry, flags, perm)
}

pub fn openat_file_with_lookup_flags(
    root: &Arc<Dentry>,
    dir: &Arc<Dentry>,
    path: &str,
    flags: FileFlags,
    perm: &Perm,
    lookup_flags: LookupFlags,
) -> SysResult<Arc<dyn FileOps>> {
    let dentry = vfs().lookup_dentry_with_perm_and_flags(root, dir, path, perm, lookup_flags)?;
    new_file(dentry, flags, perm)
}

pub fn openat_file_nofollow(
    root: &Arc<Dentry>,
    dir: &Arc<Dentry>,
    path: &str,
    flags: FileFlags,
    perm: &Perm,
) -> SysResult<Arc<dyn FileOps>> {
    let dentry = vfs().lookup_dentry_nofollow(root, dir, path)?;
    if dentry.get_inode().inode_type()? == FileType::Symlink {
        return Err(Errno::ELOOP);
    }
    new_file(dentry, flags, perm)
}

pub fn openat_file_nofollow_with_lookup_flags(
    root: &Arc<Dentry>,
    dir: &Arc<Dentry>,
    path: &str,
    flags: FileFlags,
    perm: &Perm,
    lookup_flags: LookupFlags,
) -> SysResult<Arc<dyn FileOps>> {
    let dentry = vfs().lookup_dentry_nofollow_with_perm_and_flags(root, dir, path, perm, lookup_flags)?;
    if dentry.get_inode().inode_type()? == FileType::Symlink {
        return Err(Errno::ELOOP);
    }
    new_file(dentry, flags, perm)
}

pub fn create_file(
    dir: &Arc<Dentry>,
    name: &str,
    flags: FileFlags,
    mode: Mode,
    owner: Owner,
) -> SysResult<Arc<dyn FileOps>> {
    let inode = dir.create(name, mode, owner)?;
    let dentry = Arc::new(dentry::Dentry::new(name, dir, &inode, dir.sno()));
    Ok(inode.wrap_file(Some(dentry), flags))
}

pub fn create_temp(dentry: &Arc<Dentry>, flags: FileFlags, mode: Mode) -> SysResult<Arc<dyn FileOps>> {
    let superblock = vfs().superblock_table.lock().get(dentry.sno()).ok_or(Errno::ENOENT)?;
    let inode = superblock.create_temp(mode)?;
    let dentry = Arc::new(Dentry::new("", dentry, &inode, dentry.sno()));

    Ok(Arc::new(RandomAccessFile::new(inode, dentry, flags)))
}
