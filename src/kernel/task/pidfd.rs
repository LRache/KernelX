use alloc::sync::{Arc, Weak};

use crate::fs::file::{FileFlags, FileOps};
use crate::fs::{Dentry, InodeOps, Mode};
use crate::kernel::errno::{Errno, SysResult};
use crate::kernel::event::FileEvent;
use crate::kernel::uapi::FileStat;
use crate::klib::SpinLock;

use super::PCB;

pub struct PidFile {
    pcb: Weak<PCB>,
    flags: SpinLock<FileFlags>,
}

impl PidFile {
    pub fn new(pcb: &Arc<PCB>, flags: FileFlags) -> Self {
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
        }
    }

    pub fn pcb(&self) -> Option<Arc<PCB>> {
        self.pcb.upgrade()
    }
}

impl FileOps for PidFile {
    fn read(&self, _buf: &mut [u8]) -> SysResult<usize> {
        Err(Errno::EINVAL)
    }

    fn write(&self, _buf: &[u8]) -> SysResult<usize> {
        Err(Errno::EINVAL)
    }

    fn flags(&self) -> FileFlags {
        *self.flags.lock()
    }

    fn fstat(&self) -> SysResult<FileStat> {
        let mut kstat = FileStat::empty();
        kstat.st_ino = self.pcb.upgrade().map_or(0, |pcb| pcb.pid() as u64);
        kstat.st_mode = (Mode::S_IFREG | Mode::S_IRUSR).bits();
        kstat.st_nlink = 1;
        Ok(kstat)
    }

    fn fsync(&self) -> SysResult<()> {
        Ok(())
    }

    fn get_inode(&self) -> Option<&Arc<dyn InodeOps>> {
        None
    }

    fn get_dentry(&self) -> Option<&Arc<Dentry>> {
        None
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
