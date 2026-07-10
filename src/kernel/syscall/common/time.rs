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
#[derive(Clone, Copy, Debug, UserStruct)]
pub struct Timespec {
    pub tv_sec: u64,
    pub tv_nsec: u64,
}

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
#[derive(Clone, Copy, Debug, UserStruct)]
pub struct Timeval {
    pub tv_sec: u64,
    pub tv_usec: u64,
}

impl Timeval {
    pub const ZERO: Self = Self { tv_sec: 0, tv_usec: 0 };

    pub fn is_zero(&self) -> bool {
        self.tv_sec == 0 && self.tv_usec == 0
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

impl From<Timeval> for Duration {
    fn from(value: Timeval) -> Self {
        Duration::new(value.tv_sec, (value.tv_usec * 1000) as u32)
    }
}

#[repr(C)]
#[derive(Clone, Copy, UserStruct)]
pub struct Timespec32 {
    pub tv_sec: i32,
    pub tv_nsec: i32,
}

impl TryFrom<Timespec32> for Duration {
    type Error = Errno;

    fn try_from(value: Timespec32) -> Result<Self, Self::Error> {
        checked_duration(value.tv_sec as i64, value.tv_nsec as i64)
    }
}

#[repr(C)]
#[derive(Clone, Copy, UserStruct)]
pub struct ITimerSpec {
    pub it_interval: Timespec,
    pub it_value: Timespec,
}

impl ITimerSpec {
    pub fn into_durations(self) -> SysResult<(Duration, Duration)> {
        Ok((self.it_value.try_into()?, self.it_interval.try_into()?))
    }

    pub fn from_durations(interval: Duration, value: Duration) -> Self {
        Self {
            it_interval: interval.into(),
            it_value: value.into(),
        }
    }
}

#[repr(C)]
#[derive(Clone, Copy, Debug, UserStruct)]
pub struct ITimerVal {
    pub it_interval: Timeval,
    pub it_value: Timeval,
}

impl ITimerVal {
    pub fn from_durations(interval: Duration, value: Duration) -> Self {
        Self {
            it_interval: interval.into(),
            it_value: value.into(),
        }
    }
}
