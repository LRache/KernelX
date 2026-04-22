use alloc::sync::Arc;
use num_enum::TryFromPrimitive;

use crate::driver::chosen::kclock;
use crate::fs::Dentry;
use crate::fs::file::{DirResult, FileFlags, FileOps};
use crate::fs::inode::{InodeLockState, InodeOps, Mode};
use crate::kernel::errno::{Errno, SysResult};
use crate::kernel::event::FileEvent;
use crate::kernel::mm::AddrSpace;
use crate::kernel::uapi::FileStat;
use crate::klib::SpinLock;

/// Linux-compatible `struct rtc_time`
#[repr(C)]
#[derive(Clone, Copy, Default)]
struct RtcTime {
    tm_sec: i32,
    tm_min: i32,
    tm_hour: i32,
    tm_mday: i32,
    tm_mon: i32,  // months since January [0, 11]
    tm_year: i32, // years since 1900
    tm_wday: i32, // days since Sunday [0, 6]
    tm_yday: i32, // days since January 1 [0, 365]
    tm_isdst: i32,
}

pub struct RtcInode {
    ino: u32,
    lock_state: SpinLock<InodeLockState>,
}

impl RtcInode {
    pub fn new(ino: u32) -> Self {
        Self {
            ino,
            lock_state: SpinLock::new(InodeLockState::new(), "RtcInode::lock_state"),
        }
    }
}

impl InodeOps for RtcInode {
    fn get_ino(&self) -> u32 {
        self.ino
    }

    fn lock_state(&self) -> Option<&SpinLock<InodeLockState>> {
        Some(&self.lock_state)
    }

    fn type_name(&self) -> &'static str {
        "devfs"
    }

    fn readat(&self, _buf: &mut [u8], _offset: usize, _direct: bool) -> SysResult<usize> {
        Ok(0)
    }

    fn writeat(&self, _buf: &[u8], _offset: usize) -> SysResult<usize> {
        Err(Errno::EACCES)
    }

    fn get_dent(&self, _index: usize) -> SysResult<Option<(DirResult, usize)>> {
        Err(Errno::ENOTDIR)
    }

    fn fstat(&self) -> SysResult<FileStat> {
        let mut kstat = FileStat::default();
        kstat.st_ino = self.ino as u64;
        kstat.st_size = 0;
        kstat.st_mode = Mode::S_IFCHR.bits() as u32 | 0o444;
        kstat.st_nlink = 1;
        kstat.st_gid = 0;
        kstat.st_uid = 0;
        Ok(kstat)
    }

    fn mode(&self) -> SysResult<Mode> {
        Ok(Mode::from_bits_truncate(Mode::S_IFCHR.bits() as u32 | 0o444))
    }

    fn size(&self) -> SysResult<u64> {
        Ok(0)
    }

    fn wrap_file(self: Arc<Self>, dentry: Option<Arc<Dentry>>, flags: FileFlags) -> Arc<dyn FileOps> {
        Arc::new(RtcFile {
            inode: self,
            dentry,
            flags,
        })
    }
}

struct RtcFile {
    inode: Arc<RtcInode>,
    dentry: Option<Arc<Dentry>>,
    flags: FileFlags,
}

impl FileOps for RtcFile {
    fn read(&self, buf: &mut [u8]) -> SysResult<usize> {
        self.inode.readat(buf, 0, self.flags.direct)
    }

    fn write(&self, _buf: &[u8]) -> SysResult<usize> {
        Err(Errno::EACCES)
    }

    fn flags(&self) -> FileFlags {
        FileFlags {
            readable: self.flags.readable,
            writable: false,
            blocked: self.flags.blocked,
            append: false,
            direct: self.flags.direct,
        }
    }

    fn ioctl(&self, request: usize, arg: usize, addrspace: &AddrSpace) -> SysResult<usize> {
        #[derive(TryFromPrimitive)]
        #[allow(non_camel_case_types)]
        #[repr(u32)]
        enum Request {
            RTC_RD_TIME = 0x80247009,
        }

        let request = Request::try_from_primitive(request as u32).map_err(|_| Errno::ENOTTY)?;
        match request {
            Request::RTC_RD_TIME => {
                let now = kclock::now()?;
                let epoch_secs = now.as_secs();
                let rtc_time = epoch_to_rtc_time(epoch_secs);
                addrspace.copy_to_user(arg, rtc_time)?;
                Ok(0)
            }
        }
    }

    fn fstat(&self) -> SysResult<FileStat> {
        self.inode.fstat()
    }

    fn fsync(&self) -> SysResult<()> {
        Ok(())
    }

    fn get_inode(&self) -> Option<&Arc<dyn InodeOps>> {
        None
    }

    fn get_dentry(&self) -> Option<&Arc<Dentry>> {
        self.dentry.as_ref()
    }

    fn wait_event(&self, _waker: usize, _event: FileEvent) -> SysResult<Option<FileEvent>> {
        Ok(None)
    }

    fn type_name(&self) -> &'static str {
        "RtcFile"
    }
}

unsafe impl Send for RtcFile {}
unsafe impl Sync for RtcFile {}

/// Convert Unix epoch seconds to `RtcTime`.
fn epoch_to_rtc_time(epoch: u64) -> RtcTime {
    let secs = epoch as i64;

    let secs_per_day: i64 = 86400;
    let days_since_epoch = secs.div_euclid(secs_per_day);
    let day_secs = secs.rem_euclid(secs_per_day);

    let sec = (day_secs % 60) as i32;
    let min = ((day_secs / 60) % 60) as i32;
    let hour = (day_secs / 3600) as i32;

    // days_since_epoch is relative to 1970-01-01 (Thursday, wday=4)
    let wday = ((days_since_epoch + 4) % 7) as i32;

    // Convert days since epoch to (year, yday)
    let mut days = days_since_epoch;
    let mut year: i64 = 1970;

    loop {
        let days_in_year = if is_leap(year) { 366 } else { 365 };
        if days < days_in_year {
            break;
        }
        days -= days_in_year;
        year += 1;
    }

    let yday = days as i32;

    // Convert yday to (mon, mday)
    let leap = is_leap(year);
    let mdays: [i32; 12] = [31, if leap { 29 } else { 28 }, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31];
    let mut mon = 0i32;
    let mut remaining = yday;
    for m in 0..12 {
        if remaining < mdays[m] {
            mon = m as i32;
            break;
        }
        remaining -= mdays[m];
    }

    RtcTime {
        tm_sec: sec,
        tm_min: min,
        tm_hour: hour,
        tm_mday: remaining + 1, // 1-based
        tm_mon: mon,            // 0-based
        tm_year: (year - 1900) as i32,
        tm_wday: wday,
        tm_yday: yday,
        tm_isdst: 0,
    }
}

fn is_leap(year: i64) -> bool {
    (year % 4 == 0 && year % 100 != 0) || year % 400 == 0
}
