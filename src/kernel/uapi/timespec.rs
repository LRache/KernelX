use core::time::Duration;

use crate::kernel::errno::{Errno, SysResult};
use crate::kernel::syscall::UserStruct;

fn checked_duration(tv_sec: i64, tv_nsec: i64) -> SysResult<Duration> {
    if tv_sec < 0 || tv_nsec < 0 || tv_nsec >= 1_000_000_000 {
        return Err(Errno::EINVAL);
    }

    Ok(Duration::new(tv_sec as u64, tv_nsec as u32))
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct Timespec {
    pub tv_sec: u64,  // seconds
    pub tv_nsec: u64, // nanoseconds
}

impl UserStruct for Timespec {}

impl TryFrom<Timespec> for Duration {
    type Error = Errno;

    fn try_from(value: Timespec) -> Result<Self, Self::Error> {
        checked_duration(value.tv_sec as i64, value.tv_nsec as i64)
    }
}

impl From<Duration> for Timespec {
    fn from(dur: Duration) -> Self {
        Timespec {
            tv_sec: dur.as_secs() as u64,
            tv_nsec: dur.subsec_nanos() as u64,
        }
    }
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct Timeval {
    pub tv_sec: u64,  // seconds
    pub tv_usec: u64, // microseconds
}

impl UserStruct for Timeval {}

impl Into<Duration> for Timeval {
    fn into(self) -> Duration {
        Duration::new(self.tv_sec as u64, (self.tv_usec * 1000) as u32)
    }
}

impl From<Duration> for Timeval {
    fn from(dur: Duration) -> Self {
        Timeval {
            tv_sec: dur.as_secs() as u64,
            tv_usec: (dur.subsec_nanos() / 1000) as u64,
        }
    }
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct Timespec32 {
    pub tv_sec: i32,  // seconds
    pub tv_nsec: i32, // nanoseconds
}

impl UserStruct for Timespec32 {}

impl TryFrom<Timespec32> for Duration {
    type Error = Errno;

    fn try_from(value: Timespec32) -> Result<Self, Self::Error> {
        checked_duration(value.tv_sec as i64, value.tv_nsec as i64)
    }
}

#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct TimeVal {
    pub tv_sec: usize,  // seconds
    pub tv_usec: usize, // microseconds
}

impl TimeVal {
    pub fn is_zero(&self) -> bool {
        self.tv_sec == 0 && self.tv_usec == 0
    }
}

impl From<Duration> for TimeVal {
    fn from(dur: Duration) -> Self {
        TimeVal {
            tv_sec: dur.as_secs() as usize,
            tv_usec: (dur.subsec_nanos() / 1000) as usize,
        }
    }
}

impl Into<Duration> for TimeVal {
    fn into(self) -> Duration {
        Duration::new(self.tv_sec as u64, (self.tv_usec * 1000) as u32)
    }
}
