use core::time::Duration;

use crate::kernel::syscall::UserStruct;

#[repr(C)]
#[derive(Clone, Copy)]
pub struct Timespec {
    pub tv_sec:  u64,     // seconds
    pub tv_nsec: u64,     // nanoseconds
}

impl UserStruct for Timespec {}

impl Into<Duration> for Timespec {
    fn into(self) -> Duration {
        Duration::new(self.tv_sec as u64, self.tv_nsec as u32)
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
    pub tv_sec:  u64,     // seconds
    pub tv_usec: u64,     // microseconds
}

impl UserStruct for Timeval {}

impl Into<Duration> for Timeval {
    fn into(self) -> Duration {
        Duration::new(self.tv_sec as u64, (self.tv_usec * 1000) as u32)
    }
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct Timespec32 {
    pub tv_sec:  i32,     // seconds
    pub tv_nsec: i32,     // nanoseconds
}

impl UserStruct for Timespec32 {}

impl Into<Duration> for Timespec32 {
    fn into(self) -> Duration {
        Duration::new(self.tv_sec as u64, self.tv_nsec as u32)
    }
}

#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct TimeVal {
    pub tv_sec:  usize,    // seconds
    pub tv_usec: usize,    // microseconds
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
