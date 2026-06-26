use alloc::format;
use alloc::sync::Arc;
use core::sync::atomic::{AtomicUsize, Ordering};

use crate::fs::devfs::superblock::DevfsInfo;
use crate::fs::file::{FileFlags, FileOps};
use crate::fs::inode::InodeLockState;
use crate::fs::memtreefs::inode::Inode as MemInode;
use crate::fs::{Dentry, Inode, InodeOps, Mode, memtreefs};
use crate::kernel::errno::{Errno, SysResult};
use crate::kernel::uapi::FileStat;
use crate::klib::SpinLock;

use super::file::{PtmxFile, PtsFile};
use super::inner::PtyInner;

pub struct PtmxInode {
    ino: u32,
    superblock: Arc<memtreefs::SuperBlock<DevfsInfo>>,
    pts_dir: Arc<MemInode<DevfsInfo>>,
    next_id: AtomicUsize,
    lock_state: SpinLock<InodeLockState>,
}

impl PtmxInode {
    pub fn new(ino: u32, superblock: Arc<memtreefs::SuperBlock<DevfsInfo>>, pts_dir: Arc<MemInode<DevfsInfo>>) -> Self {
        Self {
            ino,
            superblock,
            pts_dir,
            next_id: AtomicUsize::new(0),
            lock_state: SpinLock::new(InodeLockState::new(), "PtmxInode::lock_state"),
        }
    }
}

impl InodeOps for PtmxInode {
    fn get_ino(&self) -> u32 {
        self.ino
    }

    fn type_name(&self) -> &'static str {
        "devfs"
    }

    fn lock_state(&self) -> Option<&SpinLock<InodeLockState>> {
        Some(&self.lock_state)
    }

    fn readat(&self, _buf: &mut [u8], _offset: usize, _direct: bool) -> SysResult<usize> {
        Err(Errno::EINVAL)
    }

    fn writeat(&self, _buf: &[u8], _offset: usize) -> SysResult<usize> {
        Err(Errno::EINVAL)
    }

    fn fstat(&self) -> SysResult<FileStat> {
        char_fstat(self.ino, self.mode()?)
    }

    fn mode(&self) -> SysResult<Mode> {
        Ok(Mode::S_IFCHR | Mode::from_bits_truncate(0o666))
    }

    fn size(&self) -> SysResult<u64> {
        Ok(0)
    }

    fn wrap_file(
        self: Arc<Self>,
        inode: Arc<Inode>,
        dentry: Option<Arc<Dentry>>,
        flags: FileFlags,
    ) -> Arc<dyn FileOps> {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let inner = Arc::new(PtyInner::new(id, self.pts_dir.clone()));
        let slave_inode = Arc::new(PtsInode::new(self.superblock.alloc_inode_number(), inner.clone()));
        self.pts_dir
            .add_child(format!("{}", id), slave_inode)
            .expect("failed to add /dev/pts entry");

        Arc::new(PtmxFile::new(inner, inode, dentry, flags))
    }
}

struct PtsInode {
    ino: u32,
    inner: Arc<PtyInner>,
    lock_state: SpinLock<InodeLockState>,
}

impl PtsInode {
    fn new(ino: u32, inner: Arc<PtyInner>) -> Self {
        Self {
            ino,
            inner,
            lock_state: SpinLock::new(InodeLockState::new(), "PtsInode::lock_state"),
        }
    }
}

impl InodeOps for PtsInode {
    fn get_ino(&self) -> u32 {
        self.ino
    }

    fn type_name(&self) -> &'static str {
        "devfs"
    }

    fn lock_state(&self) -> Option<&SpinLock<InodeLockState>> {
        Some(&self.lock_state)
    }

    fn readat(&self, _buf: &mut [u8], _offset: usize, _direct: bool) -> SysResult<usize> {
        Err(Errno::EINVAL)
    }

    fn writeat(&self, _buf: &[u8], _offset: usize) -> SysResult<usize> {
        Err(Errno::EINVAL)
    }

    fn fstat(&self) -> SysResult<FileStat> {
        char_fstat(self.ino, self.mode()?)
    }

    fn mode(&self) -> SysResult<Mode> {
        Ok(Mode::S_IFCHR | Mode::from_bits_truncate(0o666))
    }

    fn size(&self) -> SysResult<u64> {
        Ok(0)
    }

    fn wrap_file(
        self: Arc<Self>,
        inode: Arc<Inode>,
        dentry: Option<Arc<Dentry>>,
        flags: FileFlags,
    ) -> Arc<dyn FileOps> {
        Arc::new(PtsFile::new(self.inner.clone(), inode, dentry, flags))
    }
}

fn char_fstat(ino: u32, mode: Mode) -> SysResult<FileStat> {
    let mut kstat = FileStat::default();
    kstat.st_ino = ino as u64;
    kstat.st_mode = mode.bits();
    kstat.st_nlink = 1;
    kstat.st_uid = 0;
    kstat.st_gid = 0;
    kstat.st_size = 0;
    Ok(kstat)
}
