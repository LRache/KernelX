use crate::{copy_to_user, kinfo, platform};
use crate::kernel::errno::SysResult;

#[repr(C)]
struct Timeval {
    pub tv_sec:  u64,     // seconds
    pub tv_usec: u64,     // microseconds
}

pub fn gettimeofday(uptr_timeval: usize, _uptr_tz: usize) -> SysResult<usize> {
    let us = platform::get_time_us();
    
    let timeval = Timeval {
        tv_sec:  us / 1000000,
        tv_usec: us % 1000000,
    };

    // kinfo!("gettimeofday: {}s {}us", timeval.tv_sec, timeval.tv_usec);

    copy_to_user!(uptr_timeval, timeval)?;

    Ok(0)
}