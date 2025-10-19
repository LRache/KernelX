use crate::kernel::scheduler::current;
use crate::kernel::event::{timer, Event};
use crate::kernel::errno::{SysResult, Errno};
use crate::kernel::syscall::uptr::{UPtr, UserPointer};
use crate::kernel::uapi::{Timespec, Timeval};
use crate::platform;

pub fn gettimeofday(uptr_timeval: UPtr<Timeval>, _uptr_tz: usize) -> SysResult<usize> {
    uptr_timeval.should_not_null()?;
    
    let us = platform::get_time_us();
    let timeval = Timeval {
        tv_sec:  us / 1000000,
        tv_usec: us % 1000000,
    };
    
    uptr_timeval.write(timeval)?;

    Ok(0)
}

pub fn clock_nanosleep(_clockid: usize, _flags: usize, uptr_req: UPtr<Timespec>, _uptr_rem: usize) -> SysResult<usize> {
    uptr_req.should_not_null()?;
    
    let req = uptr_req.read()?;

    if req.tv_sec == 0 && req.tv_nsec == 0 {
        return Ok(0);
    }

    let sleep_time = req.tv_sec * 1000000 + req.tv_nsec / 1000;

    let tcb = current::tcb().clone();
    tcb.block("timer nanosleep");
    timer::add_timer(tcb, sleep_time);

    current::schedule();

    let event = current::tcb().state().lock().event.unwrap();
    match event {
        Event::Timeout => Ok(0),
        Event::Signal => Err(Errno::EINTR),
        _ => unreachable!(),
    }
}
