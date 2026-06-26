use alloc::borrow::Cow;
use alloc::format;
use alloc::sync::Arc;

use crate::driver::BlockDriverOps;
use crate::fs::file::{FileFlags, FileOps, RandomAccessFile};
use crate::fs::filesystem::{FileSystemOps, MountOptions, SuperBlockOps};
use crate::fs::inode::{FileType, Index, Mode, Owner};
use crate::fs::memtreefs;
use crate::fs::perm::Perm;
use crate::fs::vfs::dentry::{self, Dentry};
use crate::kernel::errno::{Errno, SysResult};
use crate::kernel::uapi::FileSealFlags;
use crate::klib::SpinLock;

use super::{LookupFlags, vfs};

struct MemfdFsInfo;

impl memtreefs::StaticFsInfo for MemfdFsInfo {
    fn type_name() -> &'static str {
        "memfd"
    }

    fn statfs_magic() -> u64 {
        0x01021994
    }
}

struct MemfdFileSystem;

impl FileSystemOps for MemfdFileSystem {
    fn create(
        &self,
        _fsno: u32,
        _driver: Option<Arc<dyn BlockDriverOps>>,
        options: MountOptions,
    ) -> SysResult<Arc<dyn SuperBlockOps>> {
        Ok(Arc::new(memtreefs::SuperBlock::<MemfdFsInfo>::new(options.read_only)))
    }
}

static MEMFD_SUPERBLOCK_SNO: SpinLock<Option<u32>> = SpinLock::new(None, "vfs::fileop::MEMFD_SUPERBLOCK_SNO");

fn memfd_superblock_sno() -> SysResult<u32> {
    if let Some(sno) = *MEMFD_SUPERBLOCK_SNO.lock() {
        return Ok(sno);
    }

    let sno = vfs()
        .superblock_table
        .lock()
        .mount(&MemfdFileSystem, None, MountOptions::default())?;
    let mut slot = MEMFD_SUPERBLOCK_SNO.lock();
    if let Some(existing) = *slot {
        Ok(existing)
    } else {
        *slot = Some(sno);
        Ok(sno)
    }
}

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

    inode.open_file(Some(dentry), flags)
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
    inode.open_file(Some(dentry), flags)
}

pub fn create_temp(dentry: &Arc<Dentry>, flags: FileFlags, mode: Mode) -> SysResult<Arc<dyn FileOps>> {
    let superblock = vfs().superblock_table.lock().get(dentry.sno()).ok_or(Errno::ENOENT)?;
    let raw_inode = superblock.create_temp(mode)?;
    let inode = vfs().cache.insert(
        &Index {
            sno: dentry.sno(),
            ino: raw_inode.get_ino(),
        },
        raw_inode,
    )?;
    let dentry = Arc::new(Dentry::new("", dentry, &inode, dentry.sno()));

    Ok(Arc::new(RandomAccessFile::new(inode, dentry, flags)))
}

pub fn create_memfd(
    name: &str,
    flags: FileFlags,
    mode: Mode,
    initial_seals: FileSealFlags,
) -> SysResult<Arc<dyn FileOps>> {
    let sno = memfd_superblock_sno()?;
    let superblock = vfs().superblock_table.lock().get(sno).ok_or(Errno::ENOENT)?;
    let raw_inode = superblock.create_temp(mode)?;
    let inode = vfs().cache.insert(
        &Index {
            sno,
            ino: raw_inode.get_ino(),
        },
        raw_inode,
    )?;
    inode.as_seal_ops().ok_or(Errno::EINVAL)?.init_seals(initial_seals)?;

    let dentry_name = format!("memfd:{} (deleted)", name);
    let dentry = Arc::new(Dentry::new(&dentry_name, vfs().get_root(), &inode, sno));
    Ok(Arc::new(RandomAccessFile::new(inode, dentry, flags)))
}
