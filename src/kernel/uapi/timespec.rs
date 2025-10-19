#[repr(C)]
#[derive(Clone, Copy)]
pub struct Timespec {
    pub tv_sec:  u64,     // seconds
    pub tv_nsec: u64,     // nanoseconds
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct Timeval {
    pub tv_sec:  u64,     // seconds
    pub tv_usec: u64,     // microseconds
}
