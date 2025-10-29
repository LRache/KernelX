use core::time::Duration;

#[repr(C)]
#[derive(Clone, Copy)]
pub struct Timespec {
    pub tv_sec:  u64,     // seconds
    pub tv_nsec: u64,     // nanoseconds
}

impl Into<Duration> for Timespec {
    fn into(self) -> Duration {
        Duration::new(self.tv_sec as u64, self.tv_nsec as u32)
    }
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct Timeval {
    pub tv_sec:  u64,     // seconds
    pub tv_usec: u64,     // microseconds
}

impl Into<Duration> for Timeval {
    fn into(self) -> Duration {
        Duration::new(self.tv_sec as u64, (self.tv_usec * 1000) as u32)
    }
}
