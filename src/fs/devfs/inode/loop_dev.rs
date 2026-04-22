use alloc::sync::Arc;
use num_enum::TryFromPrimitive;

use crate::fs::file::{DirResult, FileFlags, FileOps, RandomAccessFile};
use crate::fs::inode::{InodeLockState, InodeOps, Mode};
use crate::fs::Dentry;
use crate::kernel::errno::{Errno, SysResult};
use crate::kernel::mm::AddrSpace;
use crate::kernel::scheduler::current;
use crate::kernel::uapi::FileStat;
use crate::klib::SpinLock;

#[derive(Default)]
struct LoopState {
    target_inode: Option<Arc<dyn InodeOps>>,
}

/// Linux-compatible `struct loop_info`
#[repr(C)]
#[derive(Clone, Copy, Debug)]
struct LoopInfo {
    lo_number: i32,
    lo_device: u32,
    lo_inode: usize,
    lo_rdevice: u32,
    lo_offset: i32,
    lo_encrypt_type: i32,
    lo_encrypt_key_size: i32,
    lo_flags: i32,
    lo_name: [u8; 64],
    lo_encrypt_key: [u8; 32],
    lo_init: [usize; 2],
    reserved: [u8; 4],
}

impl Default for LoopInfo {
    fn default() -> Self {
        Self {
            lo_number: 0,
            lo_device: 0,
            lo_inode: 0,
            lo_rdevice: 0,
            lo_offset: 0,
            lo_encrypt_type: 0,
            lo_encrypt_key_size: 0,
            lo_flags: 0,
            lo_name: [0; 64],
            lo_encrypt_key: [0; 32],
            lo_init: [0; 2],
            reserved: [0; 4],
        }
    }
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

    fn bind_target_inode(&self, target_inode: Arc<dyn InodeOps>) -> SysResult<()> {
        let mut state = self.state.lock();
        if state.target_inode.is_some() {
            return Err(Errno::EBUSY);
        }
        state.target_inode = Some(target_inode);
        Ok(())
    }

    fn clear_target_inode(&self) {
        self.state.lock().target_inode = None;
    }

    fn is_bound(&self) -> bool {
        self.state.lock().target_inode.is_some()
    }

    fn target_inode(&self) -> SysResult<Arc<dyn InodeOps>> {
        self.state.lock().target_inode.clone().ok_or(Errno::ENXIO)
    }

    fn get_status(&self, arg: usize, addrspace: &AddrSpace) -> SysResult<usize> {
        if arg == 0 {
            return Err(Errno::EINVAL);
        }
        // Linux reports ENXIO for an unbound loop device, and user space uses
        // that to detect a free /dev/loopN.
        if !self.is_bound() {
            return Err(Errno::ENXIO);
        }
        let target_inode = self.target_inode()?;
        let info = LoopInfo {
            lo_number: self.minor as i32,
            lo_inode: target_inode.get_ino() as usize,
            ..Default::default()
        };
        addrspace.copy_to_user(arg, info)?;
        Ok(0)
    }

    fn set_status(&self, arg: usize, addrspace: &AddrSpace) -> SysResult<usize> {
        if arg == 0 {
            return Err(Errno::EINVAL);
        }
        let _info: LoopInfo = addrspace.copy_from_user(arg)?;
        Ok(0)
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

    fn readat(&self, buf: &mut [u8], offset: usize, direct: bool) -> SysResult<usize> {
        self.target_inode()?.readat(buf, offset, direct)
    }

    fn writeat(&self, buf: &[u8], offset: usize) -> SysResult<usize> {
        self.target_inode()?.writeat(buf, offset)
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
        self.target_inode()?.size()
    }

    fn ioctl(&self, request: usize, arg: usize, addrspace: &AddrSpace) -> SysResult<usize> {
        #[derive(TryFromPrimitive)]
        #[allow(non_camel_case_types)]
        #[repr(usize)]
        enum Request {
            LOOP_SET_FD = 0x4C00,
            LOOP_CLR_FD = 0x4C01,
            LOOP_SET_STATUS = 0x4C02,
            LOOP_GET_STATUS = 0x4C03,
            LOOP_BLKGETSIZE64 = 0x80081272,
        }

        let request = Request::try_from_primitive(request).map_err(|_| Errno::ENOTTY)?;
        match request {
            Request::LOOP_SET_FD => {
                let backing_file = current::fdtable().lock().get(arg)?;
                let target_inode = backing_file.get_inode().cloned().ok_or(Errno::EINVAL)?;
                if target_inode.clone().downcast_arc::<LoopInode>().is_ok() {
                    return Err(Errno::EINVAL);
                }
                self.bind_target_inode(target_inode)?;
                Ok(0)
            }
            Request::LOOP_CLR_FD => {
                self.clear_target_inode();
                Ok(0)
            }
            Request::LOOP_SET_STATUS => self.set_status(arg, addrspace),
            Request::LOOP_GET_STATUS => self.get_status(arg, addrspace),
            Request::LOOP_BLKGETSIZE64 => {
                let size = self.size()?;
                addrspace.copy_to_user(arg, size)?;
                Ok(0)
            }
        }
    }

    fn wrap_file(self: Arc<Self>, dentry: Option<Arc<Dentry>>, flags: FileFlags) -> Arc<dyn FileOps> {
        let dentry = dentry.unwrap();
        Arc::new(RandomAccessFile::new(self, dentry, flags))
    }
}
