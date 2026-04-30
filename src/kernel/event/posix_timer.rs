use alloc::boxed::Box;
use alloc::sync::Arc;
use alloc::vec::Vec;
use core::time::Duration;
use num_enum::TryFromPrimitive;

use crate::driver::chosen::kclock;
use crate::kernel::errno::{Errno, SysResult};
use crate::kernel::ipc::{KSiFields, SiCode, SiTimer, SignalNum, signum};
use crate::kernel::task::{PCB, Pid, TCB};
use crate::klib::SpinLock;

use super::timer as global_timer;

#[derive(Debug, Clone, Copy, PartialEq, Eq, TryFromPrimitive)]
#[repr(usize)]
#[allow(non_camel_case_types)]
pub enum TimerClockId {
    CLOCK_REALTIME = 0,
    CLOCK_MONOTONIC = 1,
    CLOCK_PROCESS_CPUTIME_ID = 2,
    CLOCK_THREAD_CPUTIME_ID = 3,
    CLOCK_BOOTTIME = 7,
    CLOCK_REALTIME_ALARM = 8,
    CLOCK_BOOTTIME_ALARM = 9,
    CLOCK_TAI = 11,
}

impl TimerClockId {
    fn is_thread_cpu(self) -> bool {
        self == Self::CLOCK_THREAD_CPUTIME_ID
    }

    fn is_task_cpu(self) -> bool {
        matches!(self, Self::CLOCK_PROCESS_CPUTIME_ID | Self::CLOCK_THREAD_CPUTIME_ID)
    }

    fn now(self, pcb: &PCB, thread: Option<&TCB>) -> SysResult<Duration> {
        match self {
            Self::CLOCK_REALTIME | Self::CLOCK_REALTIME_ALARM | Self::CLOCK_TAI => kclock::now(),
            Self::CLOCK_MONOTONIC | Self::CLOCK_BOOTTIME | Self::CLOCK_BOOTTIME_ALARM => Ok(global_timer::now()),
            Self::CLOCK_PROCESS_CPUTIME_ID => Ok(pcb.process_cpu_time()),
            Self::CLOCK_THREAD_CPUTIME_ID => Ok(thread.ok_or(Errno::EINVAL)?.thread_cpu_time()),
        }
    }

    fn is_process_cpu(self) -> bool {
        self == Self::CLOCK_PROCESS_CPUTIME_ID
    }
}

#[derive(Clone, Copy)]
pub struct TimerTime {
    pub interval: Duration,
    pub value: Duration,
}

#[derive(Clone, Copy)]
pub enum TimerNotify {
    None,
    Signal {
        signum: SignalNum,
        sigval: usize,
        dest: Option<Pid>,
    },
}

impl Default for TimerNotify {
    fn default() -> Self {
        Self::Signal {
            signum: signum::SIGALRM,
            sigval: 0,
            dest: None,
        }
    }
}

pub struct TimerTable {
    entries: SpinLock<Vec<Option<Arc<Timer>>>>,
}

impl TimerTable {
    pub fn new() -> Self {
        Self {
            entries: SpinLock::new(Vec::new(), "TimerTable::entries"),
        }
    }

    pub fn insert(&self, f: impl FnOnce(usize) -> Arc<Timer>) -> (usize, Arc<Timer>) {
        let mut entries = self.entries.lock();
        let timerid = entries
            .iter()
            .position(|timer| timer.is_none())
            .unwrap_or(entries.len());
        let timer = f(timerid);

        if timerid == entries.len() {
            entries.push(Some(timer.clone()));
        } else {
            entries[timerid] = Some(timer.clone());
        }

        (timerid, timer)
    }

    pub fn get(&self, timerid: usize) -> Option<Arc<Timer>> {
        self.entries.lock().get(timerid).and_then(|timer| timer.clone())
    }

    pub fn remove(&self, timerid: usize) -> Option<Arc<Timer>> {
        self.entries.lock().get_mut(timerid).and_then(|timer| timer.take())
    }

    pub fn remove_if_same(&self, timerid: usize, timer: &Arc<Timer>) -> bool {
        let mut entries = self.entries.lock();
        if entries
            .get(timerid)
            .and_then(|slot| slot.as_ref())
            .map_or(false, |stored| Arc::ptr_eq(stored, timer))
        {
            entries[timerid] = None;
            true
        } else {
            false
        }
    }

    pub fn clear(&self) {
        let timers: Vec<_> = self
            .entries
            .lock()
            .iter_mut()
            .filter_map(|timer| timer.take())
            .collect();

        timers.iter().for_each(|timer| timer.delete());
    }

    pub fn check_process_cpu(&self) {
        self.snapshot().iter().for_each(|timer| timer.check_process_cpu());
    }

    pub fn check_thread_cpu(&self, tid: Pid) {
        self.snapshot().iter().for_each(|timer| timer.check_thread_cpu(tid));
    }

    fn snapshot(&self) -> Vec<Arc<Timer>> {
        self.entries.lock().iter().filter_map(|timer| timer.clone()).collect()
    }
}

#[derive(Default)]
struct TimerState {
    global_timer_id: Option<u64>,
    sequence: u64,
    interval: Duration,
    schedule: TimerSchedule,
    overrun: u64,
    deleted: bool,
}

pub struct Timer {
    id: usize,
    clock_id: TimerClockId,
    notify: TimerNotify,
    pcb: Arc<PCB>,
    thread: Option<Arc<TCB>>,
    state: SpinLock<TimerState>,
}

impl Timer {
    pub fn new(
        id: usize,
        clock_id: TimerClockId,
        notify: TimerNotify,
        pcb: Arc<PCB>,
        thread: Option<Arc<TCB>>,
    ) -> Self {
        Self {
            id,
            clock_id,
            notify,
            pcb,
            thread,
            state: SpinLock::new(TimerState::default(), "Timer::state"),
        }
    }

    pub fn get_time(&self) -> TimerTime {
        let state = self.state.lock();
        let value = match state.schedule {
            TimerSchedule::Disarmed => Duration::ZERO,
            TimerSchedule::GlobalTimer { expiry_ns } => {
                Duration::from_nanos(expiry_ns.saturating_sub(global_timer_now_ns()))
            }
            TimerSchedule::TaskCpuTimer { expiry_ns } => {
                Duration::from_nanos(expiry_ns.saturating_sub(self.task_cpu_timer_now_ns()))
            }
        };

        TimerTime {
            interval: state.interval,
            value,
        }
    }

    pub fn set_time(self: &Arc<Self>, value: Duration, interval: Duration, absolute: bool) -> SysResult<TimerTime> {
        let old = self.get_time();
        let next = if value.is_zero() {
            TimerNext {
                initial_expirations: 0,
                schedule: TimerSchedule::Disarmed,
            }
        } else if absolute {
            self.next_from_absolute(value, interval)?
        } else {
            self.next_from_relative(value)?
        };

        let (global_timer_id, sequence, initial_expirations, next_global_timer_expiry_ns) = {
            let mut state = self.state.lock();
            if state.deleted {
                return Err(Errno::EINVAL);
            }

            let global_timer_id = state.global_timer_id.take();
            state.sequence = state.sequence.wrapping_add(1);
            state.interval = interval;
            state.schedule = next.schedule;
            state.overrun = 0;

            (
                global_timer_id,
                state.sequence,
                next.initial_expirations,
                state.schedule.global_timer_expiry_ns(),
            )
        };

        if let Some(global_timer_id) = global_timer_id {
            global_timer::remove_timer(global_timer_id);
        }
        if initial_expirations != 0 {
            self.notify(initial_expirations.saturating_sub(1));
        }
        if let Some(expiry_ns) = next_global_timer_expiry_ns {
            self.arm_global_timer_at(sequence, expiry_ns);
        }

        Ok(old)
    }

    pub fn get_overrun(&self) -> usize {
        self.state.lock().overrun.min(i32::MAX as u64) as usize
    }

    pub fn set_delivered_overrun(&self, overrun: u64) {
        self.state.lock().overrun = overrun.min(i32::MAX as u64);
    }

    pub fn delete(&self) {
        let global_timer_id = {
            let mut state = self.state.lock();
            state.deleted = true;
            state.sequence = state.sequence.wrapping_add(1);
            state.schedule = TimerSchedule::Disarmed;
            state.global_timer_id.take()
        };

        if let Some(global_timer_id) = global_timer_id {
            global_timer::remove_timer(global_timer_id);
        }
    }

    fn next_from_absolute(&self, value: Duration, interval: Duration) -> SysResult<TimerNext> {
        let clock_now = self.clock_id.now(&self.pcb, self.thread.as_deref())?;
        let global_now_ns = global_timer_now_ns();

        if value > clock_now {
            let delay_ns = duration_to_ns(value - clock_now);
            let schedule = if self.clock_id.is_task_cpu() {
                TimerSchedule::TaskCpuTimer {
                    expiry_ns: duration_to_ns(value),
                }
            } else {
                TimerSchedule::GlobalTimer {
                    expiry_ns: global_now_ns.saturating_add(delay_ns),
                }
            };

            return Ok(TimerNext {
                initial_expirations: 0,
                schedule,
            });
        }

        if interval.is_zero() {
            return Ok(TimerNext {
                initial_expirations: 1,
                schedule: TimerSchedule::Disarmed,
            });
        }

        let interval_ns = duration_to_ns(interval);
        let elapsed_ns = duration_to_ns(clock_now - value);
        let missed = expirations_from_elapsed(elapsed_ns, interval_ns);
        let delay_ns = interval_ns - elapsed_ns % interval_ns;
        let schedule = if self.clock_id.is_task_cpu() {
            TimerSchedule::TaskCpuTimer {
                expiry_ns: duration_to_ns(value).saturating_add(interval_ns.saturating_mul(missed)),
            }
        } else {
            TimerSchedule::GlobalTimer {
                expiry_ns: global_now_ns.saturating_add(delay_ns),
            }
        };

        Ok(TimerNext {
            initial_expirations: missed,
            schedule,
        })
    }

    fn next_from_relative(&self, value: Duration) -> SysResult<TimerNext> {
        let delay_ns = duration_to_ns(value);
        let schedule = if self.clock_id.is_task_cpu() {
            TimerSchedule::TaskCpuTimer {
                expiry_ns: self.task_cpu_timer_now_ns().saturating_add(delay_ns),
            }
        } else {
            TimerSchedule::GlobalTimer {
                expiry_ns: global_timer_now_ns().saturating_add(delay_ns),
            }
        };

        Ok(TimerNext {
            initial_expirations: 0,
            schedule,
        })
    }

    fn task_cpu_timer_now_ns(&self) -> u64 {
        match self.clock_id {
            TimerClockId::CLOCK_PROCESS_CPUTIME_ID => process_cpu_now_ns(&self.pcb),
            TimerClockId::CLOCK_THREAD_CPUTIME_ID => self.thread.as_ref().map_or(0, |thread| thread_cpu_now_ns(thread)),
            _ => unreachable!("global timer has no task cpu time"),
        }
    }

    pub fn check_process_cpu(self: &Arc<Self>) {
        if !self.clock_id.is_process_cpu() {
            return;
        }

        self.check_task_cpu_timer();
    }

    pub fn check_thread_cpu(self: &Arc<Self>, tid: Pid) {
        if !self.clock_id.is_thread_cpu() || self.thread.as_ref().map_or(true, |thread| thread.tid() != tid) {
            return;
        }

        self.check_task_cpu_timer();
    }

    fn check_task_cpu_timer(&self) {
        let expired = {
            let mut state = self.state.lock();
            if state.deleted {
                return;
            }
            let Some(task_cpu_expiry_ns) = state.schedule.task_cpu_timer_expiry_ns() else {
                return;
            };

            let task_cpu_now_ns = self.task_cpu_timer_now_ns();
            if task_cpu_now_ns < task_cpu_expiry_ns {
                return;
            }

            if state.interval.is_zero() {
                state.schedule = TimerSchedule::Disarmed;
                state.overrun = 0;
                1
            } else {
                let interval_ns = duration_to_ns(state.interval);
                let missed = expirations_from_elapsed(task_cpu_now_ns.saturating_sub(task_cpu_expiry_ns), interval_ns);
                state.schedule = TimerSchedule::TaskCpuTimer {
                    expiry_ns: task_cpu_expiry_ns.saturating_add(interval_ns.saturating_mul(missed)),
                };
                missed
            }
        };

        self.notify(expired.saturating_sub(1));
    }

    fn arm_global_timer_at(self: &Arc<Self>, sequence: u64, expiry_ns: u64) {
        let delay_us = ns_to_timer_delay_us(expiry_ns.saturating_sub(global_timer_now_ns()));
        let timer = self.clone();
        let timer_id = global_timer::add_timer_with_callback(
            Duration::from_micros(delay_us),
            Box::new(move || {
                timer.expire_global_timer(sequence);
            }),
        );

        let mut state = self.state.lock();
        if !state.deleted
            && state.sequence == sequence
            && state.schedule.global_timer_expiry_ns() == Some(expiry_ns)
            && state.global_timer_id.is_none()
        {
            state.global_timer_id = Some(timer_id);
        } else {
            drop(state);
            global_timer::remove_timer(timer_id);
        }
    }

    fn expire_global_timer(self: Arc<Self>, sequence: u64) {
        let mut next_global_timer_expiry_ns = None;
        let expired = {
            let mut state = self.state.lock();
            if state.deleted || state.sequence != sequence {
                return;
            }
            let Some(expiry_ns) = state.schedule.global_timer_expiry_ns() else {
                return;
            };

            state.global_timer_id = None;
            let expired = if state.interval.is_zero() {
                state.schedule = TimerSchedule::Disarmed;
                1
            } else {
                let interval_ns = duration_to_ns(state.interval);
                let now_ns = global_timer_now_ns();
                let missed = expirations_from_elapsed(now_ns.saturating_sub(expiry_ns), interval_ns);
                let next = expiry_ns.saturating_add(interval_ns.saturating_mul(missed));
                state.schedule = TimerSchedule::GlobalTimer { expiry_ns: next };
                next_global_timer_expiry_ns = Some(next);
                missed
            };
            expired
        };

        self.notify(expired.saturating_sub(1));

        if let Some(expiry_ns) = next_global_timer_expiry_ns {
            self.arm_global_timer_at(sequence, expiry_ns);
        }
    }

    fn notify(&self, overrun: u64) {
        if self.pcb.is_exited() {
            return;
        }

        let TimerNotify::Signal { signum, sigval, dest } = self.notify else {
            return;
        };

        let fields = KSiFields::Timer(SiTimer {
            si_tid: self.id.min(i32::MAX as usize) as i32,
            si_overrun: overrun.min(i32::MAX as u64) as i32,
            si_sigval: sigval,
        });
        let _ = self.pcb.send_signal(signum, SiCode::SI_TIMER, 0, fields, dest);
    }
}

impl Drop for Timer {
    fn drop(&mut self) {
        self.delete();
    }
}

struct TimerNext {
    initial_expirations: u64,
    schedule: TimerSchedule,
}

#[derive(Clone, Copy, Default)]
enum TimerSchedule {
    #[default]
    Disarmed,
    GlobalTimer {
        expiry_ns: u64,
    },
    TaskCpuTimer {
        expiry_ns: u64,
    },
}

impl TimerSchedule {
    fn global_timer_expiry_ns(self) -> Option<u64> {
        match self {
            Self::GlobalTimer { expiry_ns } => Some(expiry_ns),
            _ => None,
        }
    }

    fn task_cpu_timer_expiry_ns(self) -> Option<u64> {
        match self {
            Self::TaskCpuTimer { expiry_ns } => Some(expiry_ns),
            _ => None,
        }
    }
}

fn global_timer_now_ns() -> u64 {
    duration_to_ns(global_timer::now())
}

fn process_cpu_now_ns(pcb: &PCB) -> u64 {
    duration_to_ns(pcb.process_cpu_time())
}

fn thread_cpu_now_ns(thread: &TCB) -> u64 {
    duration_to_ns(thread.thread_cpu_time())
}

fn duration_to_ns(duration: Duration) -> u64 {
    duration.as_nanos().min(u64::MAX as u128) as u64
}

fn expirations_from_elapsed(elapsed_ns: u64, interval_ns: u64) -> u64 {
    (elapsed_ns / interval_ns).saturating_add(1)
}

fn ns_to_timer_delay_us(ns: u64) -> u64 {
    if ns == 0 {
        0
    } else {
        ((ns as u128).saturating_add(999) / 1000).min(u64::MAX as u128) as u64
    }
}
