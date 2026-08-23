use alloc::sync::Arc;
use core::ffi::{CStr, c_char, c_void};
use core::time::Duration;

use crate::driver::{BlockDriverOps, CharDriverOps};
use crate::fs::file::{DirResult, FileFlags, FileOps, RandomAccessFile};
use crate::fs::{Dentry, FileType, Inode, InodeOps, Mode, Owner, Perm};
use crate::kernel::errno::{Errno, SysResult};
use crate::kernel::mm::swappable::FileMapping;
use crate::kernel::mm::{AddrSpace, PhysPageFrame};
use crate::kernel::uapi::{FileFallocateFlags, FileStat, Uid};

use super::decode_result;

#[repr(C)]
#[derive(Clone, Copy)]
/// C ABI callbacks used by [`BridgeInode`].
///
/// `size`, `mode`, `owner`, `readat`, and `writeat` return a nonnegative result
/// on success or a negative errno on failure. `type_name` returns a static
/// NUL-terminated string. Every callback receives the inode's `data` pointer as
/// its first argument.
pub struct BridgeInodeOps {
    pub type_name: unsafe extern "C" fn(data: *mut c_void) -> *const c_char,
    pub size: unsafe extern "C" fn(data: *mut c_void) -> isize,
    pub mode: unsafe extern "C" fn(data: *mut c_void) -> isize,
    pub owner: unsafe extern "C" fn(data: *mut c_void, uid: *mut Uid, gid: *mut Uid) -> isize,
    pub readat: unsafe extern "C" fn(data: *mut c_void, buf: *mut u8, len: usize, offset: usize, direct: bool) -> isize,
    pub writeat: unsafe extern "C" fn(data: *mut c_void, buf: *const u8, len: usize, offset: usize) -> isize,
}

impl BridgeInodeOps {
    /// # Safety
    ///
    /// `data` and the `type_name` callback must satisfy the lifetime contract
    /// documented by [`BridgeInode::new`].
    pub(super) unsafe fn decode_type_name(&self, data: *mut c_void) -> &'static str {
        // SAFETY: The caller guarantees that the callback and data are valid.
        let name = unsafe { (self.type_name)(data) };
        if name.is_null() {
            return "kmodule_bridge";
        }

        // SAFETY: The type_name ABI requires a pointer to an immutable,
        // NUL-terminated string that remains valid for the inode's lifetime.
        unsafe { CStr::from_ptr(name) }.to_str().unwrap_or("kmodule_bridge")
    }
}

pub struct BridgeInode {
    ino: u32,
    ops: BridgeInodeOps,
    data: *mut c_void,
}

impl BridgeInode {
    /// # Safety
    ///
    /// Every callback and any memory reachable through `data` must remain
    /// valid for the lifetime of this inode. `type_name` must return a static
    /// NUL-terminated string, and the I/O callbacks must not access beyond the
    /// provided buffer. The callbacks must also synchronize concurrent access
    /// to `data`.
    pub const unsafe fn new(ino: u32, ops: BridgeInodeOps, data: *mut c_void) -> Self {
        Self { ino, ops, data }
    }
}

// SAFETY: BridgeInode::new requires the C implementation to synchronize all
// access to data and keep the callback table and data alive.
unsafe impl Send for BridgeInode {}
// SAFETY: BridgeInode::new requires the C implementation to synchronize all
// access to data and keep the callback table and data alive.
unsafe impl Sync for BridgeInode {}

impl InodeOps for BridgeInode {
    fn attach_file_mapping(&mut self, _mapping: Arc<FileMapping>) {}

    fn get_ino(&self) -> u32 {
        self.ino
    }

    fn type_name(&self) -> &'static str {
        // SAFETY: BridgeInode::new guarantees that the callback and data satisfy
        // decode_type_name's lifetime contract.
        unsafe { self.ops.decode_type_name(self.data) }
    }

    fn block_driver(&self) -> SysResult<Option<Arc<dyn BlockDriverOps>>> {
        Err(Errno::EOPNOTSUPP)
    }

    fn char_driver(&self) -> SysResult<Option<Arc<dyn CharDriverOps>>> {
        Err(Errno::EOPNOTSUPP)
    }

    fn create(&self, _name: &str, _mode: Mode, _owner: Owner) -> SysResult<Self> {
        Err(Errno::EOPNOTSUPP)
    }

    fn mknod(&self, _name: &str, _mode: Mode, _owner: Owner, _dev: u64) -> SysResult<Self> {
        Err(Errno::EOPNOTSUPP)
    }

    fn link(&self, _name: &str, _target: &Self) -> SysResult<()> {
        Err(Errno::EOPNOTSUPP)
    }

    fn unlink(&self, _name: &str) -> SysResult<()> {
        Err(Errno::EOPNOTSUPP)
    }

    fn rmdir(&self, _name: &str) -> SysResult<()> {
        Err(Errno::EOPNOTSUPP)
    }

    fn symlink(&self, _target: &str) -> SysResult<()> {
        Err(Errno::EOPNOTSUPP)
    }

    fn readat(&self, buf: &mut [u8], offset: usize, direct: bool) -> SysResult<usize> {
        // SAFETY: BridgeInode::new guarantees that the callback and data are
        // valid, and buf supplies a writable region of exactly buf.len() bytes.
        let result = unsafe { (self.ops.readat)(self.data, buf.as_mut_ptr(), buf.len(), offset, direct) };
        let result = decode_result(result)?;
        if result > buf.len() {
            return Err(Errno::EIO);
        }
        Ok(result)
    }

    fn writeat(&self, buf: &[u8], offset: usize) -> SysResult<usize> {
        // SAFETY: BridgeInode::new guarantees that the callback and data are
        // valid, and buf supplies a readable region of exactly buf.len() bytes.
        let result = unsafe { (self.ops.writeat)(self.data, buf.as_ptr(), buf.len(), offset) };
        let result = decode_result(result)?;
        if result > buf.len() {
            return Err(Errno::EIO);
        }
        Ok(result)
    }

    fn get_dent(&self, _index: usize) -> SysResult<Option<(DirResult, usize)>> {
        Err(Errno::EOPNOTSUPP)
    }

    fn lookup(&self, _name: &str) -> SysResult<u32> {
        Err(Errno::EOPNOTSUPP)
    }

    fn rename(&self, _old_name: &str, _source: &Self, _new_parent: &Self, _new_name: &str) -> SysResult<()> {
        Err(Errno::EOPNOTSUPP)
    }

    fn readlink(&self, _buf: &mut [u8]) -> SysResult<Option<usize>> {
        Ok(None)
    }

    fn follow_magic_link(&self) -> SysResult<Option<Arc<Dentry>>> {
        Ok(None)
    }

    fn size(&self) -> SysResult<u64> {
        // SAFETY: BridgeInode::new guarantees that the callback and data are valid.
        let result = unsafe { (self.ops.size)(self.data) };
        decode_result(result).map(|size| size as u64)
    }

    fn mmap_shared_page(&self, _file_page_index: usize) -> SysResult<Option<Arc<PhysPageFrame>>> {
        Err(Errno::EOPNOTSUPP)
    }

    fn load_raw_page(&self, _file_page_index: usize) -> SysResult<Option<PhysPageFrame>> {
        Err(Errno::EOPNOTSUPP)
    }

    fn supports_file_mapping(&self) -> bool {
        false
    }

    fn writeback_mmap_shared_page(&self, _file_page_index: usize, _frame: &PhysPageFrame) -> SysResult<()> {
        Err(Errno::EOPNOTSUPP)
    }

    fn mode(&self) -> SysResult<Mode> {
        // SAFETY: BridgeInode::new guarantees that the callback and data are valid.
        let result = unsafe { (self.ops.mode)(self.data) };
        let mode = u32::try_from(decode_result(result)?).map_err(|_| Errno::EINVAL)?;
        Mode::from_bits(mode).ok_or(Errno::EINVAL)
    }

    fn check_perm(&self, perm: &Perm) -> SysResult<bool> {
        let (uid, gid) = self.owner()?;
        Ok(self.mode()?.check_perm(perm, uid, gid))
    }

    fn chmod(&self, _mode: Mode) -> SysResult<()> {
        Err(Errno::EOPNOTSUPP)
    }

    fn owner(&self) -> SysResult<(Uid, Uid)> {
        let mut uid = 0;
        let mut gid = 0;
        // SAFETY: BridgeInode::new guarantees that the callback and data are
        // valid, and uid/gid point to writable Uid values for this call.
        let result = unsafe { (self.ops.owner)(self.data, &mut uid, &mut gid) };
        decode_result(result)?;
        Ok((uid, gid))
    }

    fn chown(&self, _uid: Option<Uid>, _gid: Option<Uid>) -> SysResult<()> {
        Err(Errno::EOPNOTSUPP)
    }

    fn inode_type(&self) -> SysResult<FileType> {
        self.mode().map(Into::into)
    }

    fn sync(&self) -> SysResult<()> {
        Err(Errno::EOPNOTSUPP)
    }

    fn ioctl(&self, _request: usize, _arg: usize, _addrspace: &AddrSpace) -> SysResult<usize> {
        Err(Errno::EOPNOTSUPP)
    }

    fn fstat(&self) -> SysResult<FileStat> {
        Err(Errno::EOPNOTSUPP)
    }

    fn truncate(&self, _new_size: u64) -> SysResult<()> {
        Err(Errno::EOPNOTSUPP)
    }

    fn fallocate(&self, _flags: FileFallocateFlags, _offset: u64, _len: u64) -> SysResult<()> {
        Err(Errno::EOPNOTSUPP)
    }

    fn update_atime(&self, _time: &Duration) -> SysResult<()> {
        Err(Errno::EOPNOTSUPP)
    }

    fn update_mtime(&self, _time: &Duration) -> SysResult<()> {
        Err(Errno::EOPNOTSUPP)
    }

    fn update_ctime(&self, _time: &Duration) -> SysResult<()> {
        Err(Errno::EOPNOTSUPP)
    }

    fn update_mtime_ctime(&self, _time: &Duration) -> SysResult<()> {
        Err(Errno::EOPNOTSUPP)
    }

    fn wrap_file(&self, inode: Arc<Inode>, dentry: Option<Arc<Dentry>>, flags: FileFlags) -> Arc<dyn FileOps> {
        Arc::new(RandomAccessFile::new(
            inode,
            dentry.expect("BridgeInode requires a dentry"),
            flags,
        ))
    }
}
