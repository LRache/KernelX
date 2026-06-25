use alloc::sync::{Arc, Weak};

use crate::fs::file::{DirResult, FileFlags, FileOps, RandomAccessFile, SeekWhence};
use crate::fs::{Dentry, InodeOps, Mode};
use crate::kernel::errno::{Errno, SysResult};
use crate::kernel::event::FileEvent;
use crate::kernel::mm::ubuf::UAddrSpaceBuffer;
use crate::kernel::uapi::FileStat;
use crate::klib::SpinLock;

use super::PCB;

pub struct PidFile {
    pcb: Weak<PCB>,
    flags: SpinLock<FileFlags>,
    file: Option<Arc<RandomAccessFile>>,
}

impl PidFile {
    pub fn new(pcb: &Arc<PCB>, flags: FileFlags) -> Self {
        Self::with_file(pcb, None, flags)
    }

    pub fn new_with_file(pcb: &Arc<PCB>, file: Arc<RandomAccessFile>, flags: FileFlags) -> Self {
        Self::with_file(pcb, Some(file), flags)
    }

    fn with_file(pcb: &Arc<PCB>, file: Option<Arc<RandomAccessFile>>, flags: FileFlags) -> Self {
        Self {
            pcb: Arc::downgrade(pcb),
            flags: SpinLock::new(
                FileFlags {
                    readable: true,
                    writable: false,
                    blocked: flags.blocked,
                    append: false,
                    direct: false,
                },
                "PidFile::flags",
            ),
            file,
        }
    }

    pub fn pcb(&self) -> Option<Arc<PCB>> {
        self.pcb.upgrade()
    }
}

impl FileOps for PidFile {
    fn read(&self, buf: &mut [u8]) -> SysResult<usize> {
        if let Some(file) = &self.file {
            return file.read(buf);
        }
        Err(Errno::EINVAL)
    }

    fn write(&self, buf: &[u8]) -> SysResult<usize> {
        if let Some(file) = &self.file {
            return file.write(buf);
        }
        Err(Errno::EINVAL)
    }

    fn seek(&self, offset: isize, whence: SeekWhence) -> SysResult<usize> {
        self.file.as_ref().ok_or(Errno::ESPIPE)?.seek(offset, whence)
    }

    fn pread(&self, buf: &mut [u8], offset: usize) -> SysResult<usize> {
        self.file.as_ref().ok_or(Errno::ESPIPE)?.pread(buf, offset)
    }

    fn pread_to_user(&self, ubuf: &UAddrSpaceBuffer, offset: usize) -> SysResult<usize> {
        self.file.as_ref().ok_or(Errno::ESPIPE)?.pread_to_user(ubuf, offset)
    }

    fn pwrite(&self, buf: &[u8], offset: usize) -> SysResult<usize> {
        self.file.as_ref().ok_or(Errno::ESPIPE)?.pwrite(buf, offset)
    }

    fn pwrite_from_user(&self, ubuf: &UAddrSpaceBuffer, offset: usize) -> SysResult<usize> {
        self.file.as_ref().ok_or(Errno::ESPIPE)?.pwrite_from_user(ubuf, offset)
    }

    fn flags(&self) -> FileFlags {
        *self.flags.lock()
    }

    fn fstat(&self) -> SysResult<FileStat> {
        if let Some(file) = &self.file {
            return file.fstat();
        }
        let mut kstat = FileStat::empty();
        kstat.st_ino = self.pcb.upgrade().map_or(0, |pcb| pcb.pid() as u64);
        kstat.st_mode = (Mode::S_IFREG | Mode::S_IRUSR).bits();
        kstat.st_nlink = 1;
        Ok(kstat)
    }

    fn fsync(&self) -> SysResult<()> {
        if let Some(file) = &self.file {
            return file.fsync();
        }
        Ok(())
    }

    fn ftruncate(&self, new_size: u64) -> SysResult<()> {
        self.file.as_ref().ok_or(Errno::EINVAL)?.ftruncate(new_size)
    }

    fn get_inode(&self) -> Option<&Arc<dyn InodeOps>> {
        self.file.as_ref().and_then(|file| file.get_inode())
    }

    fn get_dentry(&self) -> Option<&Arc<Dentry>> {
        self.file.as_ref().and_then(|file| file.get_dentry())
    }

    fn get_dent(&self) -> SysResult<Option<(DirResult, usize)>> {
        self.file.as_ref().ok_or(Errno::ESPIPE)?.get_dent()
    }

    fn wait_event(&self, waker: usize, event: FileEvent) -> SysResult<Option<FileEvent>> {
        match self.pcb.upgrade() {
            Some(pcb) => Ok(pcb.wait_pidfd_event(waker, event)),
            None => Ok(event.contains(FileEvent::READ_READY).then_some(FileEvent::READ_READY)),
        }
    }

    fn wait_event_cancel(&self) {
        if let Some(pcb) = self.pcb.upgrade() {
            pcb.wait_pidfd_event_cancel();
        }
    }

    fn set_flags(&self, flags: FileFlags) {
        *self.flags.lock() = FileFlags {
            readable: true,
            writable: false,
            blocked: flags.blocked,
            append: false,
            direct: false,
        };
    }

    fn type_name(&self) -> &'static str {
        "pidfd"
    }
}
