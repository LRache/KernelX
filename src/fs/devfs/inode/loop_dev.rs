use alloc::sync::Arc;
use num_enum::TryFromPrimitive;

use crate::fs::Dentry;
use crate::fs::file::{DirResult, FileFlags, FileOps, RandomAccessFile};
use crate::fs::inode::{InodeLockState, InodeOps, Mode};
use crate::kernel::errno::{Errno, SysResult};
use crate::kernel::mm::AddrSpace;
use crate::kernel::scheduler::current;
use crate::kernel::uapi::FileStat;
use crate::klib::SpinLock;

#[derive(Default)]
struct LoopState {
    backing_file: Option<Arc<dyn FileOps>>,
}

pub struct LoopInode {
    ino: u32,
    minor: u32,
    lock_state: SpinLock<InodeLockState>,
    state: SpinLock<LoopState>,
}

impl LoopInode {
    pub fn new(ino: u32, minor: u32) -> Self {
        Self {
            ino,
            minor,
            lock_state: SpinLock::new(InodeLockState::new(), "LoopInode::lock_state"),
            state: SpinLock::new(LoopState::default(), "LoopInode::state"),
        }
    }

    fn rdev(&self) -> u64 {
        // Linux loop device: major 7
        ((7u64) << 8) | self.minor as u64
    }

    fn bind_backing_file(&self, backing_file: Arc<dyn FileOps>) -> SysResult<()> {
        let mut state = self.state.lock();
        if state.backing_file.is_some() {
            return Err(Errno::EBUSY);
        }
        state.backing_file = Some(backing_file);
        Ok(())
    }

    fn clear_backing_file(&self) {
        self.state.lock().backing_file = None;
    }
}

impl InodeOps for LoopInode {
    fn get_ino(&self) -> u32 {
        self.ino
    }

    fn lock_state(&self) -> Option<&SpinLock<InodeLockState>> {
        Some(&self.lock_state)
    }

    fn type_name(&self) -> &'static str {
        "devfs"
    }

    fn readat(&self, buf: &mut [u8], _offset: usize, _direct: bool) -> SysResult<usize> {
        buf.fill(0);
        Ok(buf.len())
    }

    fn writeat(&self, _buf: &[u8], _offset: usize) -> SysResult<usize> {
        Err(Errno::ENODEV)
    }

    fn get_dent(&self, _index: usize) -> SysResult<Option<(DirResult, usize)>> {
        Err(Errno::ENOTDIR)
    }

    fn fstat(&self) -> SysResult<FileStat> {
        let mut kstat = FileStat::default();
        kstat.st_ino = self.ino as u64;
        kstat.st_size = 0;
        kstat.st_mode = Mode::S_IFBLK.bits() as u32 | 0o660;
        kstat.st_nlink = 1;
        kstat.st_rdev = self.rdev();
        Ok(kstat)
    }

    fn mode(&self) -> SysResult<Mode> {
        Ok(Mode::from_bits_truncate(Mode::S_IFBLK.bits() as u32 | 0o660))
    }

    fn size(&self) -> SysResult<u64> {
        Ok(0)
    }

    fn wrap_file(self: Arc<Self>, dentry: Option<Arc<Dentry>>, flags: FileFlags) -> Arc<dyn FileOps> {
        let dentry = dentry.unwrap();
        Arc::new(LoopFile {
            inode: self.clone(),
            inner: RandomAccessFile::new(self, dentry, flags),
        })
    }
}

struct LoopFile {
    inode: Arc<LoopInode>,
    inner: RandomAccessFile,
}

impl FileOps for LoopFile {
    fn read(&self, buf: &mut [u8]) -> SysResult<usize> {
        self.inner.read(buf)
    }

    fn write(&self, buf: &[u8]) -> SysResult<usize> {
        self.inner.write(buf)
    }

    fn flags(&self) -> FileFlags {
        self.inner.flags
    }

    fn ioctl(&self, request: usize, arg: usize, _addrspace: &AddrSpace) -> SysResult<usize> {
        #[derive(TryFromPrimitive)]
        #[allow(non_camel_case_types)]
        #[repr(usize)]
        enum Request {
            LOOP_SET_FD = 0x4C00,
            LOOP_CLR_FD = 0x4C01,
        }

        let request = Request::try_from_primitive(request).map_err(|_| Errno::ENOTTY)?;
        match request {
            Request::LOOP_SET_FD => {
                let backing_file = current::fdtable().lock().get(arg)?;
                let backing_inode = backing_file.get_inode().ok_or(Errno::EINVAL)?;
                if backing_inode.clone().downcast_arc::<LoopInode>().is_ok() {
                    return Err(Errno::EINVAL);
                }
                self.inode.bind_backing_file(backing_file)?;
                Ok(0)
            }
            Request::LOOP_CLR_FD => {
                self.inode.clear_backing_file();
                Ok(0)
            }
        }
    }

    fn fstat(&self) -> SysResult<FileStat> {
        self.inner.fstat()
    }

    fn fsync(&self) -> SysResult<()> {
        self.inner.fsync()
    }

    fn get_inode(&self) -> Option<&Arc<dyn InodeOps>> {
        self.inner.get_inode()
    }

    fn get_dentry(&self) -> Option<&Arc<Dentry>> {
        self.inner.get_dentry()
    }

    fn on_fd_install(&self) -> SysResult<()> {
        self.inner.on_fd_install()
    }

    fn on_fd_remove(&self) {
        self.inner.on_fd_remove();
    }

    fn type_name(&self) -> &'static str {
        "LoopFile"
    }
}
