use alloc::sync::Arc;
use bitflags::bitflags;
use core::convert::{TryFrom, TryInto};
use core::time::Duration;
use num_enum::TryFromPrimitive;

use crate::driver::chosen::kclock;
use crate::kernel::errno::{Errno, SysResult};
use crate::kernel::event::{Event, Timer, TimerClockId, TimerNotify, timer};
use crate::kernel::ipc::SignalNum;
use crate::kernel::scheduler::current;
use crate::kernel::syscall::uptr::{UPtr, UserPointer, UserStruct};

use super::common::{ITimerSpec, Timespec, Timeval};
use super::uid::{self, Capability};

#[repr(C)]
#[derive(Clone, Copy)]
pub struct SetTimeval {
    tv_sec: i64,
    tv_usec: i64,
}

impl UserStruct for SetTimeval {}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct SetTimespec {
    tv_sec: i64,
    tv_nsec: i64,
}

impl UserStruct for SetTimespec {}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct Timezone {
    tz_minuteswest: i32,
    tz_dsttime: i32,
}

impl UserStruct for Timezone {}

pub fn gettimeofday(uptr_timeval: UPtr<Timeval>, uptr_tz: UPtr<u8>) -> SysResult<usize> {
    if !uptr_tz.is_null() {
        uptr_tz.read()?;
    }
    uptr_timeval.should_not_null()?;

    let timeval = kclock::now()?.into();
    uptr_timeval.write(timeval)?;

    Ok(0)
}

pub fn clock_settime(clockid: usize, uptr_timespec: UPtr<SetTimespec>) -> SysResult<usize> {
    let clockid = ClockId::try_from(clockid).map_err(|_| Errno::EINVAL)?;
    if clockid != ClockId::CLOCK_REALTIME {
        return Err(Errno::EINVAL);
    }

    let timespec = uptr_timespec.should_not_null()?.read()?;
    if timespec.tv_sec < 0 || timespec.tv_nsec < 0 || timespec.tv_nsec >= 1_000_000_000 {
        return Err(Errno::EINVAL);
    }

    if !uid::capable(Capability::SysTime) {
        return Err(Errno::EPERM);
    }

    let time = Duration::new(timespec.tv_sec as u64, timespec.tv_nsec as u32);
    kclock::set_time(time)?;

    Ok(0)
}

pub fn settimeofday(uptr_timeval: UPtr<SetTimeval>, uptr_timezone: UPtr<Timezone>) -> SysResult<usize> {
    let mut time = None;
    if !uptr_timeval.is_null() {
        let timeval = uptr_timeval.read()?;
        if timeval.tv_sec < 0 || timeval.tv_usec < 0 || timeval.tv_usec >= 1_000_000 {
            return Err(Errno::EINVAL);
        }
        time = Some(Duration::new(timeval.tv_sec as u64, (timeval.tv_usec * 1000) as u32));
    }

    if !uptr_timezone.is_null() {
        uptr_timezone.read()?;
    }

    if !uid::capable(Capability::SysTime) {
        return Err(Errno::EPERM);
    }

    if let Some(time) = time {
        kclock::set_time(time)?;
    }

    Ok(0)
}

pub fn nanosleep(uptr_req: UPtr<Timespec>, uptr_rem: UPtr<Timespec>) -> SysResult<usize> {
    uptr_req.should_not_null()?;

    let req = uptr_req.read()?;

    if req.tv_sec == 0 && req.tv_nsec == 0 {
        return Ok(0);
    }

    let to_sleep = Duration::try_from(req)?;

    let start_sleep = timer::now();
    let timer_id = timer::add_timer(current::task().clone(), to_sleep);
    let event = current::block("timer nanosleep");

    match event {
        Event::Timeout => Ok(0),
        Event::Signal => {
            if !uptr_rem.is_null() {
                let elapsed = timer::now() - start_sleep;
                let remaining = to_sleep.checked_sub(elapsed).unwrap_or(Duration::ZERO);
                uptr_rem.write(remaining.into())?;
            }
            timer::remove_timer(timer_id);
            Err(Errno::EINTR)
        }
        _ => unreachable!("event={:?}", event),
    }
}

bitflags! {
    pub struct ClockNanosleepFlags: usize {
        const TIMER_ABSTIME = 0x1;
    }
}

bitflags! {
    pub struct TimerSetTimeFlags: usize {
        const TIMER_ABSTIME = 0x1;
    }
}

#[derive(TryFromPrimitive, Debug, PartialEq, Eq)]
#[repr(usize)]
#[allow(non_camel_case_types)]
enum ClockId {
    CLOCK_REALTIME = 0,
    CLOCK_MONOTONIC = 1,
    CLOCK_PROCESS_CPUTIME_ID = 2,
    CLOCK_THREAD_CPUTIME_ID = 3,
    CLOCK_MONOTONIC_RAW = 4,
    CLOCK_REALTIME_COARSE = 5,
    CLOCK_MONOTONIC_COARSE = 6,
    CLOCK_BOOTTIME = 7,
    CLOCK_REALTIME_ALARM = 8,
    CLOCK_BOOTTIME_ALARM = 9,
    CLOCK_TAI = 11,
}

impl ClockId {
    fn now(&self) -> SysResult<Duration> {
        match self {
            ClockId::CLOCK_REALTIME
            | ClockId::CLOCK_REALTIME_COARSE
            | ClockId::CLOCK_REALTIME_ALARM
            | ClockId::CLOCK_TAI => kclock::now(),
            ClockId::CLOCK_MONOTONIC
            | ClockId::CLOCK_MONOTONIC_RAW
            | ClockId::CLOCK_BOOTTIME
            | ClockId::CLOCK_BOOTTIME_ALARM
            | ClockId::CLOCK_MONOTONIC_COARSE => Ok(timer::now()),
            ClockId::CLOCK_PROCESS_CPUTIME_ID => Ok(current::pcb().process_cpu_time()),
            ClockId::CLOCK_THREAD_CPUTIME_ID => Ok(current::tcb().thread_cpu_time()),
        }
    }
}

pub fn clock_nanosleep(
    clockid: usize,
    flags: usize,
    uptr_req: UPtr<Timespec>,
    uptr_rem: UPtr<Timespec>,
) -> SysResult<usize> {
    uptr_req.should_not_null()?;

    let clockid = ClockId::try_from(clockid).map_err(|_| Errno::EINVAL)?;
    if clockid == ClockId::CLOCK_PROCESS_CPUTIME_ID || clockid == ClockId::CLOCK_THREAD_CPUTIME_ID {
        return Err(Errno::EOPNOTSUPP);
    }

    let flags = ClockNanosleepFlags::from_bits(flags).ok_or(Errno::EINVAL)?;

    let req: Duration = uptr_req.read()?.try_into()?;

    let to_sleep = if flags.contains(ClockNanosleepFlags::TIMER_ABSTIME) {
        let now = clockid.now()?;
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

    let start_sleep = timer::now();
    let timer_id = timer::add_timer(current::task().clone(), to_sleep);
    let event = current::block("timer nanosleep");

    match event {
        Event::Timeout => Ok(0),
        Event::Signal => {
            if !uptr_rem.is_null() && !flags.contains(ClockNanosleepFlags::TIMER_ABSTIME) {
                let elapsed = timer::now() - start_sleep;
                let remaining = to_sleep.checked_sub(elapsed).unwrap_or(Duration::ZERO);
                uptr_rem.write(remaining.into())?;
            }
            timer::remove_timer(timer_id);
            Err(Errno::EINTR)
        }
        _ => unreachable!(),
    }
}

pub fn clock_gettime(clockid: usize, uptr_timespec: UPtr<Timespec>) -> SysResult<usize> {
    uptr_timespec.should_not_null()?;

    let clockid = ClockId::try_from(clockid).map_err(|_| Errno::EINVAL)?;
    let timespec = clockid.now()?.into();

    uptr_timespec.write(timespec)?;

    Ok(0)
}

pub fn clock_getres(_clockid: usize, uptr_timespec: UPtr<Timespec>) -> SysResult<usize> {
    if !uptr_timespec.is_null() {
        // Report 1ns resolution
        let res = Timespec { tv_sec: 0, tv_nsec: 1 };
        uptr_timespec.write(res)?;
    }
    Ok(0)
}

#[repr(i32)]
#[derive(Debug, TryFromPrimitive)]
#[allow(non_camel_case_types)]
enum SigEventNotify {
    SIGEV_SIGNAL = 0,
    SIGEV_NONE = 1,
    SIGEV_THREAD = 2,
    SIGEV_THREAD_ID = 4,
}

const SIGEV_PAD_SIZE: usize = 64 - core::mem::size_of::<usize>() - 3 * core::mem::size_of::<i32>();

#[repr(C)]
#[derive(Clone, Copy)]
pub struct SigEvent {
    sigev_value: usize,
    sigev_signo: i32,
    sigev_notify: i32,
    sigev_notify_thread_id: i32,
    _pad: [u8; SIGEV_PAD_SIZE],
}

impl UserStruct for SigEvent {}

impl SigEvent {
    fn notify(self) -> SysResult<TimerNotify> {
        match SigEventNotify::try_from(self.sigev_notify).map_err(|_| Errno::EINVAL)? {
            SigEventNotify::SIGEV_SIGNAL => Ok(TimerNotify::Signal {
                signum: SignalNum::try_from(self.sigev_signo as u32)?,
                sigval: self.sigev_value,
                dest: None,
            }),
            SigEventNotify::SIGEV_NONE => Ok(TimerNotify::None),
            SigEventNotify::SIGEV_THREAD => Err(Errno::EOPNOTSUPP),
            SigEventNotify::SIGEV_THREAD_ID => Ok(TimerNotify::Signal {
                signum: SignalNum::try_from(self.sigev_signo as u32)?,
                sigval: self.sigev_value,
                dest: Some(self.sigev_notify_thread_id),
            }),
        }
    }
}

fn timerid_to_index(timerid: usize) -> SysResult<usize> {
    let timerid = timerid as i32;
    if timerid < 0 {
        return Err(Errno::EINVAL);
    }
    Ok(timerid as usize)
}

fn get_timer(timerid: usize) -> SysResult<Arc<Timer>> {
    let timerid = timerid_to_index(timerid)?;
    current::pcb().timers.get(timerid).ok_or(Errno::EINVAL)
}

pub fn timer_create(clockid: usize, uptr_sev: UPtr<SigEvent>, uptr_timerid: UPtr<u32>) -> SysResult<usize> {
    if uptr_timerid.is_null() {
        return Err(Errno::EFAULT);
    }

    let clock_id = TimerClockId::try_from(clockid).map_err(|_| Errno::EINVAL)?;
    let notify = uptr_sev
        .read_optional()?
        .map(|sigevent| sigevent.notify())
        .transpose()?
        .unwrap_or_default();
    let pcb = current::pcb().clone();
    let thread = if clock_id == TimerClockId::CLOCK_THREAD_CPUTIME_ID {
        let tid = current::tid();
        Some(
            pcb.tasks
                .lock()
                .iter()
                .find(|thread| thread.tid() == tid)
                .cloned()
                .ok_or(Errno::EINVAL)?,
        )
    } else {
        None
    };

    let pcb_for_timer = pcb.clone();
    let (timerid, timer) = pcb
        .timers
        .insert(|timerid| Arc::new(Timer::new(timerid, clock_id, notify, pcb_for_timer, thread)));

    if let Err(err) = uptr_timerid.write(timerid as u32) {
        pcb.timers.remove_if_same(timerid, &timer);
        timer.delete();
        return Err(err);
    }

    Ok(0)
}

pub fn timer_settime(
    timerid: usize,
    flags: usize,
    uptr_new_value: UPtr<ITimerSpec>,
    uptr_old_value: UPtr<ITimerSpec>,
) -> SysResult<usize> {
    uptr_new_value.should_not_null()?;

    let flags = TimerSetTimeFlags::from_bits(flags).ok_or(Errno::EINVAL)?;
    let (value, interval) = uptr_new_value.read()?.into_durations()?;
    let timer = get_timer(timerid)?;
    let old = timer.set_time(value, interval, flags.contains(TimerSetTimeFlags::TIMER_ABSTIME))?;
    if !uptr_old_value.is_null() {
        uptr_old_value.write(ITimerSpec::from_durations(old.interval, old.value))?;
    }

    Ok(0)
}

pub fn timer_gettime(timerid: usize, uptr_value: UPtr<ITimerSpec>) -> SysResult<usize> {
    if uptr_value.is_null() {
        return Err(Errno::EFAULT);
    }

    let timer = get_timer(timerid)?;
    let time = timer.get_time();
    uptr_value.write(ITimerSpec::from_durations(time.interval, time.value))?;

    Ok(0)
}

pub fn timer_getoverrun(timerid: usize) -> SysResult<usize> {
    Ok(get_timer(timerid)?.get_overrun())
}

pub fn timer_delete(timerid: usize) -> SysResult<usize> {
    let timerid = timerid_to_index(timerid)?;
    let timer = current::pcb().timers.remove(timerid).ok_or(Errno::EINVAL)?;
    timer.delete();

    Ok(0)
}
