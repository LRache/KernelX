use core::time::Duration;
use bitflags::bitflags;

use crate::driver::chosen::kclock;
use crate::kernel::scheduler::current;
use crate::kernel::event::{timer, Event};
use crate::kernel::errno::{SysResult, Errno};
use crate::kernel::syscall::uptr::{UserPointer, UPtr};
use crate::kernel::uapi::{Timespec, Timeval};
use crate::driver;

pub fn gettimeofday(uptr_timeval: UPtr<Timeval>, _uptr_tz: usize) -> SysResult<usize> {
    uptr_timeval.should_not_null()?;
    
    let timeval = kclock::now()?.into();
    
    uptr_timeval.write(timeval)?;

    Ok(0)
}

pub fn nanosleep(uptr_req: UPtr<Timespec>, uptr_rem: UPtr<Timespec>) -> SysResult<usize> {
    uptr_req.should_not_null()?;
    
    let req = uptr_req.read()?;

    if req.tv_sec == 0 && req.tv_nsec == 0 {
        return Ok(0);
    }

    let to_sleep = req.into();

    let start_sleep = kclock::now()?;
    timer::add_timer(current::task().clone(), to_sleep);
    let event = current::block("timer nanosleep");
    
    match event {
        Event::Timeout => Ok(0),
        Event::Signal => {
            if !uptr_rem.is_null() {
                let elapsed = kclock::now()? - start_sleep;
                let remaining = to_sleep.checked_sub(elapsed).unwrap_or(Duration::ZERO);
                uptr_rem.write(remaining.into())?;
            }
            Err(Errno::EINTR)
        },
        _ => unreachable!("event={:?}", event),
    }
}

bitflags! {
    pub struct ClockNanosleepFlags: usize {
        const TIMER_ABSTIME = 0x1;
    }
}

pub fn clock_nanosleep(_clockid: usize, flags: usize, uptr_req: UPtr<Timespec>, uptr_rem: UPtr<Timespec>) -> SysResult<usize> {
    uptr_req.should_not_null()?;
    
    let flags = ClockNanosleepFlags::from_bits(flags).ok_or(Errno::EINVAL)?;

    let req: Duration = uptr_req.read()?.into();

    let to_sleep = if flags.contains(ClockNanosleepFlags::TIMER_ABSTIME) {
        let now = kclock::now()?;
        if req <= now {
            return Ok(0);
        }
        req - now
    } else {
        if req == Duration::ZERO {
            return Ok(0);
        }
        req
    };

    let start_sleep = kclock::now()?;
    timer::add_timer(current::task().clone(), to_sleep);
    let event = current::block("timer nanosleep");
    
    match event {
        Event::Timeout => Ok(0),
        Event::Signal => {
            if !uptr_rem.is_null() && !flags.contains(ClockNanosleepFlags::TIMER_ABSTIME) {
                let elapsed = kclock::now()? - start_sleep;
                let remaining = to_sleep.checked_sub(elapsed).unwrap_or(Duration::ZERO);
                uptr_rem.write(remaining.into())?;
            }
            Err(Errno::EINTR)
        },
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
