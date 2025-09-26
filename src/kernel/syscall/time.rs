use crate::kernel::scheduler::current;
use crate::kernel::timer;
use crate::{copy_from_user, copy_to_user, platform};
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

    copy_to_user!(uptr_timeval, timeval)?;

    Ok(0)
}

#[repr(C)]
struct Timespec {
    pub tv_sec:  u64,     // seconds
    pub tv_nsec: u64,     // nanoseconds
}

pub fn clock_nanosleep(_clockid: usize, _flags: usize, uptr_req: usize, _uptr_rem: usize) -> SysResult<usize> {
    let req: Timespec = Timespec { tv_sec: 0, tv_nsec: 0 };
    copy_from_user!(uptr_req, req)?;

    if req.tv_sec == 0 && req.tv_nsec == 0 {
        return Ok(0);
    }

    let sleep_time = req.tv_sec * 1000000 + req.tv_nsec / 1000;

    let tcb = current::tcb().clone();
    tcb.block();
    timer::add_timer(tcb, sleep_time);

    current::schedule();

    Ok(0)
}
