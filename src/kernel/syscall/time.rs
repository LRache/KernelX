use crate::kernel::scheduler::current;
use crate::kernel::event::{timer, Event};
use crate::kernel::errno::{SysResult, Errno};
use crate::kernel::syscall::uptr::{UserPointer, UPtr};
use crate::kernel::uapi::{Timespec, Timeval};
use crate::arch;

pub fn gettimeofday(uptr_timeval: UPtr<Timeval>, _uptr_tz: usize) -> SysResult<usize> {
    uptr_timeval.should_not_null()?;
    
    let us = arch::get_time_us();
    let timeval = Timeval {
        tv_sec:  us / 1000000,
        tv_usec: us % 1000000,
    };
    
    uptr_timeval.write(timeval)?;

    Ok(0)
}

pub fn nanosleep(uptr_req: UPtr<Timespec>, _uptr_rem: usize) -> SysResult<usize> {
    uptr_req.should_not_null()?;
    
    let req = uptr_req.read()?;

    if req.tv_sec == 0 && req.tv_nsec == 0 {
        return Ok(0);
    }

    timer::add_timer(current::task().clone(), req.into());
    let event = current::block("timer nanosleep");
    
    match event {
        Event::Timeout => Ok(0),
        Event::Signal => Err(Errno::EINTR),
        _ => unreachable!(),
    }
}

pub fn clock_nanosleep(_clockid: usize, _flags: usize, uptr_req: UPtr<Timespec>, _uptr_rem: usize) -> SysResult<usize> {
    uptr_req.should_not_null()?;
    
    let req = uptr_req.read()?;

    if req.tv_sec == 0 && req.tv_nsec == 0 {
        return Ok(0);
    }

    timer::add_timer(current::task().clone(), req.into());
    let event = current::block("timer nanosleep");
    
    match event {
        Event::Timeout => Ok(0),
        Event::Signal => Err(Errno::EINTR),
        _ => unreachable!(),
    }
}

pub fn clock_gettime(_clockid: usize, uptr_timespec: UPtr<Timespec>) -> SysResult<usize> {
    uptr_timespec.should_not_null()?;
    
    let us = arch::get_time_us();
    let timespec = Timespec {
        tv_sec:  us / 1000000,
        tv_nsec: (us % 1000000) * 1000,
    };
    
    uptr_timespec.write(timespec)?;

    Ok(0)
}
