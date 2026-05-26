use alloc::boxed::Box;
use alloc::sync::Arc;
use core::sync::atomic::{AtomicUsize, Ordering};

use crate::fs::file::DirResult;
use crate::fs::inode::release_bsd_flock;
use crate::fs::vfs::Dentry;
use crate::fs::{InodeOps, Mode};
use crate::kernel::errno::{Errno, SysResult};
use crate::kernel::event::FileEvent;
use crate::kernel::ipc::{signum, KSiFields, SiCode};
use crate::kernel::mm::maparea::{Area, PrivateFileMapArea, SharedFileMapArea};
use crate::kernel::mm::ubuf::UAddrSpaceBuffer;
use crate::kernel::mm::{AddrSpace, MapPerm};
use crate::kernel::scheduler::current;
use crate::kernel::uapi::{FileSealFlags, FileStat};
use crate::klib::SleepLock;

use super::{FileMmapRequest, FileOps, SeekWhence};

#[derive(Clone, Copy)]
pub struct FileFlags {
    pub readable: bool,
    pub writable: bool,
    pub blocked: bool,
    pub append: bool,
    pub direct: bool,
}

impl FileFlags {
    pub const fn readonly() -> Self {
        FileFlags {
            readable: true,
            writable: false,
            blocked: true,
            append: false,
            direct: false,
        }
    }
}

pub struct RandomAccessFile {
    inode: Arc<dyn InodeOps>,
    dentry: Arc<Dentry>,
    pos: SleepLock<usize>,
    fd_refs: AtomicUsize,

    pub flags: FileFlags,
}

impl RandomAccessFile {
    pub fn new(inode: Arc<dyn InodeOps>, dentry: Arc<Dentry>, flags: FileFlags) -> Self {
        Self {
            inode,
            dentry,
            pos: SleepLock::new(0, "RandomAccessFile::pos"),
            fd_refs: AtomicUsize::new(0),
            flags,
        }
    }

    pub fn read_at(&self, buf: &mut [u8], offset: usize) -> SysResult<usize> {
        self.pread(buf, offset)
    }

    pub fn pread(&self, buf: &mut [u8], offset: usize) -> SysResult<usize> {
        self.inode.readat(buf, offset, self.flags.direct)
    }

    pub fn pwrite(&self, buf: &[u8], mut offset: usize) -> SysResult<usize> {
        if self.flags.append {
            offset = self.inode.size()? as usize;
        }
        let len = self.limit_write_len(offset, buf.len())?;
        self.inode.writeat(&buf[..len], offset)
    }

    pub fn ftruncate(&self, new_size: u64) -> SysResult<()> {
        self.inode.truncate(new_size)
    }

    pub fn ioctl(&self, request: usize, arg: usize, addrspace: &AddrSpace) -> SysResult<usize> {
        self.inode.ioctl(request, arg, addrspace)
    }

    /// Return the dirent and the old file pos.
    pub fn get_dent(&self) -> SysResult<Option<(DirResult, usize)>> {
        let mut pos = self.pos.lock();
        let old_pos = *pos;
        let (mut dent, next_pos) = match self.inode.get_dent(*pos)? {
            Some(d) => d,
            None => return Ok(None),
        };
        *pos = next_pos;

        if dent.name == ".." {
            if let Some(parent) = self.dentry.get_parent() {
                dent.ino = parent.get_inode().get_ino();
            }
        }

        Ok(Some((dent, old_pos)))
    }

    pub fn mode(&self) -> SysResult<Mode> {
        self.inode.mode()
    }

    pub fn owner(&self) -> SysResult<(u32, u32)> {
        self.inode.owner()
    }

    pub fn seek(&self, offset: isize, whence: SeekWhence) -> SysResult<usize> {
        let mut pos = self.pos.lock();
        let new_pos = match whence {
            SeekWhence::BEG => {
                if offset < 0 {
                    return Err(Errno::EINVAL);
                }
                offset as isize
            }
            SeekWhence::CUR => {
                if offset < 0 && (*pos as isize + offset) < 0 {
                    return Err(Errno::EINVAL);
                }
                *pos as isize + offset
            }
            SeekWhence::END => {
                let size = self.inode.size()?;
                if offset > 0 && (size as isize + offset) < 0 {
                    return Err(Errno::EINVAL);
                }
                size as isize + offset
            }
        };

        if new_pos < 0 {
            return Err(Errno::EINVAL);
        }
        *pos = new_pos as usize;

        Ok(*pos)
    }

    fn release_bsd_flock_if_last_fd(&self) {
        let previous = self.fd_refs.fetch_sub(1, Ordering::AcqRel);
        debug_assert!(previous > 0, "RandomAccessFile::fd_refs underflow");
        if previous == 1 {
            release_bsd_flock(&self.inode, self.flock_owner_id());
        }
    }

    fn limit_write_len(&self, offset: usize, len: usize) -> SysResult<usize> {
        let (rlim_cur, _) = current::pcb().file_size_limit();
        if len == 0 || rlim_cur == usize::MAX {
            return Ok(len);
        }

        if (self.inode.mode()? & Mode::S_IFMT) != Mode::S_IFREG {
            return Ok(len);
        }

        if offset >= rlim_cur {
            let _ = current::pcb().send_signal(signum::SIGXFSZ, SiCode::SI_KERNEL, 0, KSiFields::Empty, None);
            return Err(Errno::EFBIG);
        }

        Ok(core::cmp::min(len, rlim_cur - offset))
    }
}

impl FileOps for RandomAccessFile {
    fn read(&self, buf: &mut [u8]) -> SysResult<usize> {
        let mut pos = self.pos.lock();
        let len = self.inode.readat(buf, *pos, self.flags.direct)?;
        *pos += len;

        Ok(len)
    }

    fn read_to_user(&self, ubuf: &UAddrSpaceBuffer) -> SysResult<usize> {
        let mut pos = self.pos.lock();
        let len = self.inode.read_to_user(ubuf, *pos, self.flags.direct)?;
        *pos += len;
        Ok(len)
    }

    fn write(&self, buf: &[u8]) -> SysResult<usize> {
        let mut pos = self.pos.lock();
        if self.flags.append {
            let size = self.inode.size()?;

            *pos = size as usize;
        }
        let len = self.limit_write_len(*pos, buf.len())?;
        let len = self.inode.writeat(&buf[..len], *pos)?;
        *pos += len;

        Ok(len)
    }

    fn write_from_user(&self, ubuf: &UAddrSpaceBuffer) -> SysResult<usize> {
        let mut pos = self.pos.lock();
        if self.flags.append {
            *pos = self.inode.size()? as usize;
        }
        let limit_len = self.limit_write_len(*pos, ubuf.length())?;
        let ubuf = ubuf.with_length(limit_len);
        let len = self.inode.write_from_user(&ubuf, *pos, self.flags.direct)?;
        *pos += len;
        Ok(len)
    }

    fn flags(&self) -> FileFlags {
        self.flags
    }

    fn ioctl(&self, request: usize, arg: usize, addrspace: &AddrSpace) -> SysResult<usize> {
        RandomAccessFile::ioctl(self, request, arg, addrspace)
    }

    fn fstat(&self) -> SysResult<FileStat> {
        self.inode.fstat()
    }

    fn fsync(&self) -> SysResult<()> {
        self.inode.sync()
    }

    fn get_inode(&self) -> Option<&Arc<dyn InodeOps>> {
        Some(&self.inode)
    }

    fn get_dentry(&self) -> Option<&Arc<Dentry>> {
        Some(&self.dentry)
    }

    fn mmap_area(self: Arc<Self>, request: FileMmapRequest) -> SysResult<Box<dyn Area>> {
        if !self.flags.readable {
            return Err(Errno::EACCES);
        }

        if request.shared {
            if request.perm.contains(MapPerm::W) {
                if !self.flags.writable {
                    return Err(Errno::EACCES);
                }

                if let Some(seal_ops) = self.inode.as_seal_ops() {
                    if let Ok(seals) = seal_ops.seals() {
                        if seals.intersects(FileSealFlags::F_SEAL_WRITE | FileSealFlags::F_SEAL_FUTURE_WRITE) {
                            return Err(Errno::EPERM);
                        }
                    }
                }
            }

            Ok(Box::new(SharedFileMapArea::new(
                0,
                request.perm,
                self.inode.clone(),
                self.dentry.get_inode_index(),
                request.offset,
                request.length,
                self.flags.writable,
                self.dentry.get_path(),
            )))
        } else {
            Ok(Box::new(PrivateFileMapArea::new(
                0,
                request.perm,
                self,
                request.offset,
                request.length,
            )))
        }
    }

    fn poll_event(&self, event: FileEvent) -> SysResult<Option<FileEvent>> {
        let mut ready = FileEvent::empty();

        if event.contains(FileEvent::READ_READY) && self.flags.readable {
            ready |= FileEvent::READ_READY;
        }
        if event.contains(FileEvent::WRITE_READY) && self.flags.writable {
            ready |= FileEvent::WRITE_READY;
        }

        if ready.is_empty() {
            Ok(None)
        } else {
            Ok(Some(ready))
        }
    }

    fn wait_event(&self, _waker: usize, event: FileEvent) -> SysResult<Option<FileEvent>> {
        self.poll_event(event)
    }

    fn on_fd_install(&self) -> SysResult<()> {
        if self.flags.writable {
            self.inode.begin_write_open()?;
        }
        self.fd_refs.fetch_add(1, Ordering::Relaxed);
        Ok(())
    }

    fn on_fd_remove(&self) {
        if self.flags.writable {
            self.inode.end_write_open();
        }
        self.release_bsd_flock_if_last_fd();
    }

    fn type_name(&self) -> &'static str {
        self.inode.type_name()
    }
}
