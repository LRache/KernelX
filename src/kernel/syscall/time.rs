use crate::kernel::scheduler::current;
use crate::kernel::event::{timer, Event};
use crate::kernel::errno::{SysResult, Errno};
use crate::kernel::syscall::uptr::{UserPointer, UPtr};
use crate::kernel::uapi::{Timespec, Timeval};
use crate::driver;

pub fn gettimeofday(uptr_timeval: UPtr<Timeval>, _uptr_tz: usize) -> SysResult<usize> {
    uptr_timeval.should_not_null()?;
    
    let timeval = driver::chosen::kclock::now()?.into();
    
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
        _ => unreachable!("event={:?}", event),
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
    
    let timespec = driver::chosen::kclock::now()?.into();
    
    uptr_timespec.write(timespec)?;

    Ok(0)
}

pub fn timer_create(_clockid: usize, _uptr_sev: usize, _uptr_timerid: UPtr<usize>) -> SysResult<usize> {
    Ok(0)
}

pub fn timer_settime(_timerid: usize, _flags: usize, _uptr_new_value: UPtr<Timespec>, _uptr_old_value: UPtr<Timespec>) -> SysResult<usize> {
    Ok(0)
}

pub fn timer_gettime(_timerid: usize, _uptr_value: UPtr<Timespec>) -> SysResult<usize> {
    Ok(0)
}

pub fn timer_getoverrun(_timerid: usize) -> SysResult<usize> {
    Ok(0)
}

pub fn timer_delete(_timerid: usize) -> SysResult<usize> {
    Ok(0)
}
