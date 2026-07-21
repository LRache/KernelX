use alloc::boxed::Box;
use alloc::sync::Arc;
use core::sync::atomic::{AtomicUsize, Ordering};

use crate::fs::Mode;
use crate::fs::file::DirResult;
use crate::fs::inode::{Inode, release_bsd_flock};
use crate::fs::vfs::Dentry;
use crate::kernel::errno::{Errno, SysResult};
use crate::kernel::event::FileEvent;
use crate::kernel::ipc::{KSiFields, SiCode, signum};
use crate::kernel::mm::maparea::{Area, PrivateFileMapArea, SharedFileMapArea};
use crate::kernel::mm::ubuf::UAddrSpaceBuffer;
use crate::kernel::mm::{AddrSpace, MapPerm};
use crate::kernel::scheduler::current;
use crate::kernel::uapi::{FileSealFlags, FileStat};
use crate::klib::{SleepLock, SpinLock};

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
    inode: Arc<Inode>,
    dentry: Arc<Dentry>,
    pos: SleepLock<usize>,
    fd_refs: AtomicUsize,

    flags: SpinLock<FileFlags>,
}

impl RandomAccessFile {
    pub fn new(inode: Arc<Inode>, dentry: Arc<Dentry>, flags: FileFlags) -> Self {
        Self {
            inode,
            dentry,
            pos: SleepLock::new(0, "RandomAccessFile::pos"),
            fd_refs: AtomicUsize::new(0),
            flags: SpinLock::new(flags, "RandomAccessFile::flags"),
        }
    }

    pub fn mode(&self) -> SysResult<Mode> {
        self.inode.mode()
    }

    pub fn owner(&self) -> SysResult<(u32, u32)> {
        self.inode.owner()
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
        let len = self.inode.readat(buf, *pos, self.flags().direct)?;
        *pos += len;

        Ok(len)
    }

    fn read_to_user(&self, ubuf: &UAddrSpaceBuffer) -> SysResult<usize> {
        let mut pos = self.pos.lock();
        let len = self.inode.read_to_user(ubuf, *pos, self.flags().direct)?;
        *pos += len;
        Ok(len)
    }

    fn write(&self, buf: &[u8]) -> SysResult<usize> {
        let mut pos = self.pos.lock();
        if self.flags().append {
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
        let flags = self.flags();
        if flags.append {
            *pos = self.inode.size()? as usize;
        }
        let limit_len = self.limit_write_len(*pos, ubuf.length())?;
        let ubuf = ubuf.with_length(limit_len);
        let len = self.inode.write_from_user(&ubuf, *pos, flags.direct)?;
        *pos += len;
        Ok(len)
    }

    fn seek(&self, offset: isize, whence: SeekWhence) -> SysResult<usize> {
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

    fn pread(&self, buf: &mut [u8], offset: usize) -> SysResult<usize> {
        self.inode.readat(buf, offset, self.flags().direct)
    }

    fn pread_to_user(&self, ubuf: &UAddrSpaceBuffer, offset: usize) -> SysResult<usize> {
        let direct = self.flags().direct;
        let mut total_read = 0;
        let mut current_offset = offset;
        for kbuf in ubuf.iter_mut() {
            let mut kbuf = match kbuf {
                Ok(kbuf) => kbuf,
                Err(_) if total_read > 0 => return Ok(total_read),
                Err(err) => return Err(err),
            };
            let n = match self.inode.readat(&mut kbuf, current_offset, direct) {
                Ok(n) => n,
                Err(_) if total_read > 0 => return Ok(total_read),
                Err(err) => return Err(err),
            };
            total_read += n;
            current_offset += n;
            if n < kbuf.len() {
                return Ok(total_read);
            }
        }
        Ok(total_read)
    }

    fn pwrite(&self, buf: &[u8], mut offset: usize) -> SysResult<usize> {
        let flags = self.flags();
        if flags.append {
            offset = self.inode.size()? as usize;
        }
        let len = self.limit_write_len(offset, buf.len())?;
        self.inode.writeat(&buf[..len], offset)
    }

    fn pwrite_from_user(&self, ubuf: &UAddrSpaceBuffer, mut offset: usize) -> SysResult<usize> {
        let flags = self.flags();
        if flags.append {
            offset = self.inode.size()? as usize;
        }
        let limit_len = self.limit_write_len(offset, ubuf.length())?;
        let ubuf = ubuf.with_length(limit_len);
        let mut total_written = 0;
        let mut current_offset = offset;
        for kbuf in ubuf.iter() {
            let kbuf = match kbuf {
                Ok(kbuf) => kbuf,
                Err(_) if total_written > 0 => return Ok(total_written),
                Err(err) => return Err(err),
            };
            let n = match self.inode.writeat(&kbuf, current_offset) {
                Ok(n) => n,
                Err(_) if total_written > 0 => return Ok(total_written),
                Err(err) => return Err(err),
            };
            total_written += n;
            current_offset += n;
            if n < kbuf.len() {
                return Ok(total_written);
            }
        }
        Ok(total_written)
    }

    fn flags(&self) -> FileFlags {
        *self.flags.lock()
    }

    fn set_flags(&self, flags: FileFlags) {
        *self.flags.lock() = flags;
    }

    fn ioctl(&self, request: usize, arg: usize, addrspace: &AddrSpace) -> SysResult<usize> {
        self.inode.ioctl(request, arg, addrspace)
    }

    fn fstat(&self) -> SysResult<FileStat> {
        self.inode.fstat()
    }

    fn fsync(&self) -> SysResult<()> {
        self.inode.sync()
    }

    fn ftruncate(&self, new_size: u64) -> SysResult<()> {
        self.inode.truncate(new_size)
    }

    fn get_inode(&self) -> Option<&Arc<Inode>> {
        Some(&self.inode)
    }

    fn get_dentry(&self) -> Option<&Arc<Dentry>> {
        Some(&self.dentry)
    }

    /// Return the dirent and the old file pos.
    fn get_dent(&self) -> SysResult<Option<(DirResult, usize)>> {
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

    fn mmap_area(self: Arc<Self>, request: FileMmapRequest) -> SysResult<Box<dyn Area>> {
        let flags = self.flags();
        if !flags.readable {
            return Err(Errno::EACCES);
        }

        if request.shared {
            if request.perm.contains(MapPerm::W) {
                if !flags.writable {
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
                flags.writable,
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
        let flags = self.flags();

        if event.contains(FileEvent::READ_READY) && flags.readable {
            ready |= FileEvent::READ_READY;
        }
        if event.contains(FileEvent::WRITE_READY) && flags.writable {
            ready |= FileEvent::WRITE_READY;
        }

        if ready.is_empty() { Ok(None) } else { Ok(Some(ready)) }
    }

    fn wait_event(&self, _waker: usize, event: FileEvent) -> SysResult<Option<FileEvent>> {
        self.poll_event(event)
    }

    fn on_fd_install(&self) -> SysResult<()> {
        if self.flags().writable {
            self.inode.begin_write_open()?;
        }
        self.fd_refs.fetch_add(1, Ordering::Relaxed);
        Ok(())
    }

    fn on_fd_remove(&self) {
        if self.flags().writable {
            self.inode.end_write_open();
        }
        self.release_bsd_flock_if_last_fd();
    }

    fn clone_file(&self) -> Arc<dyn FileOps> {
        Arc::new(RandomAccessFile {
            inode: self.inode.clone(),
            dentry: self.dentry.clone(),
            pos: SleepLock::new(0, "RandomAccessFile::pos"),
            fd_refs: AtomicUsize::new(0),
            flags: SpinLock::new(self.flags(), "RandomAccessFile::flags"),
        })
    }

    fn type_name(&self) -> &'static str {
        self.inode.type_name()
    }
}
