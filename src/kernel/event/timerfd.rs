use alloc::boxed::Box;
use alloc::sync::Arc;
use core::mem::size_of;
use core::time::Duration;
use num_enum::TryFromPrimitive;

use crate::driver::chosen::kclock;
use crate::fs::file::{FileFlags, FileOps};
use crate::fs::{Dentry, InodeOps, Mode};
use crate::kernel::errno::{Errno, SysResult};
use crate::kernel::event::{EpollNotifier, Event, FileEvent, WaitQueue, timer};
use crate::kernel::mm::ubuf::UAddrSpaceBuffer;
use crate::kernel::scheduler::current;
use crate::kernel::scheduler::current::copy_to_user;
use crate::kernel::uapi::FileStat;
use crate::klib::SpinLock;

#[derive(Debug, Clone, Copy, PartialEq, Eq, TryFromPrimitive)]
#[repr(usize)]
#[allow(non_camel_case_types)]
pub enum TimerFdClockId {
    CLOCK_REALTIME = 0,
    CLOCK_MONOTONIC = 1,
    CLOCK_BOOTTIME = 7,
    CLOCK_REALTIME_ALARM = 8,
    CLOCK_BOOTTIME_ALARM = 9,
}

impl TimerFdClockId {
    fn now(self) -> SysResult<Duration> {
        match self {
            Self::CLOCK_REALTIME | Self::CLOCK_REALTIME_ALARM => kclock::now(),
            Self::CLOCK_MONOTONIC | Self::CLOCK_BOOTTIME | Self::CLOCK_BOOTTIME_ALARM => Ok(timer::now()),
        }
    }
}

#[derive(Clone, Copy)]
pub struct TimerFdTime {
    pub interval: Duration,
    pub value: Duration,
}

#[derive(Default)]
struct TimerFdState {
    timer_id: Option<u64>,
    sequence: u64,
    interval: Duration,
    next_expiry_us: Option<u64>,
    expirations: u64,
}

struct TimerFdInner {
    state: SpinLock<TimerFdState>,
    waiter: SpinLock<WaitQueue<Event>>,
    epoll_notifier: Arc<EpollNotifier>,
}

impl TimerFdInner {
    fn new() -> Self {
        Self {
            state: SpinLock::new(TimerFdState::default(), "TimerFdInner::state"),
            waiter: SpinLock::new(WaitQueue::new(), "TimerFdInner::waiter"),
            epoll_notifier: Arc::new(EpollNotifier::new()),
        }
    }

    fn arm_at(self: &Arc<Self>, sequence: u64, expiry_us: u64) {
        let delay_us = expiry_us.saturating_sub(now_us());
        let inner = self.clone();
        let timer_id = timer::add_timer_with_callback(
            Duration::from_micros(delay_us),
            Box::new(move || {
                inner.expire(sequence);
            }),
        );

        let mut state = self.state.lock();
        if state.sequence == sequence && state.next_expiry_us == Some(expiry_us) && state.timer_id.is_none() {
            state.timer_id = Some(timer_id);
        } else {
            drop(state);
            timer::remove_timer(timer_id);
        }
    }

    fn expire(self: Arc<Self>, sequence: u64) {
        let mut next_expiry_us = None;
        let should_wake = {
            let mut state = self.state.lock();
            if state.sequence != sequence {
                return;
            }
            let Some(expiry_us) = state.next_expiry_us else {
                return;
            };

            state.timer_id = None;
            let expired = if state.interval.is_zero() {
                state.next_expiry_us = None;
                1
            } else {
                let interval_us = duration_to_us(state.interval);
                let now_us = now_us();
                let missed = now_us.saturating_sub(expiry_us) / interval_us + 1;
                let next = expiry_us.saturating_add(interval_us.saturating_mul(missed));
                state.next_expiry_us = Some(next);
                next_expiry_us = Some(next);
                missed
            };

            state.expirations = state.expirations.saturating_add(expired);
            true
        };

        if should_wake {
            self.waiter.lock().wake_all(|event| event);
            self.epoll_notifier.notify(FileEvent::READ_READY);
        }

        if let Some(expiry_us) = next_expiry_us {
            self.arm_at(sequence, expiry_us);
        }
    }

    fn disarm(&self) {
        let timer_id = {
            let mut state = self.state.lock();
            state.sequence = state.sequence.wrapping_add(1);
            state.next_expiry_us = None;
            state.timer_id.take()
        };
        if let Some(timer_id) = timer_id {
            timer::remove_timer(timer_id);
        }
    }
}

pub struct TimerFd {
    inner: Arc<TimerFdInner>,
    clock_id: TimerFdClockId,
    flags: SpinLock<FileFlags>,
}

impl TimerFd {
    const IO_BYTES: usize = size_of::<u64>();

    pub fn new(clock_id: TimerFdClockId, blocked: bool) -> Self {
        Self {
            inner: Arc::new(TimerFdInner::new()),
            clock_id,
            flags: SpinLock::new(
                FileFlags {
                    readable: true,
                    writable: false,
                    blocked,
                    append: false,
                    direct: false,
                },
                "TimerFd::flags",
            ),
        }
    }

    pub fn get_time(&self) -> TimerFdTime {
        let state = self.inner.state.lock();
        let value = state
            .next_expiry_us
            .map(|expiry_us| Duration::from_micros(expiry_us.saturating_sub(now_us())))
            .unwrap_or(Duration::ZERO);

        TimerFdTime {
            interval: state.interval,
            value,
        }
    }

    pub fn set_time(&self, value: Duration, interval: Duration, absolute: bool) -> SysResult<TimerFdTime> {
        let old = self.get_time();
        let next = if value.is_zero() {
            TimerFdNext::Disarmed
        } else if absolute {
            self.next_from_absolute(value, interval)?
        } else {
            TimerFdNext::Armed {
                initial_expirations: 0,
                next_expiry_us: Some(now_us().saturating_add(duration_to_us(value))),
            }
        };

        let (timer_id, sequence, initial_expirations, next_expiry_us) = {
            let mut state = self.inner.state.lock();
            let timer_id = state.timer_id.take();
            state.sequence = state.sequence.wrapping_add(1);
            state.interval = interval;
            state.expirations = 0;

            let (initial_expirations, next_expiry_us) = match next {
                TimerFdNext::Disarmed => {
                    state.next_expiry_us = None;
                    (0, None)
                }
                TimerFdNext::Armed {
                    initial_expirations,
                    next_expiry_us,
                } => {
                    state.expirations = initial_expirations;
                    state.next_expiry_us = next_expiry_us;
                    (initial_expirations, next_expiry_us)
                }
            };

            (timer_id, state.sequence, initial_expirations, next_expiry_us)
        };

        if let Some(timer_id) = timer_id {
            timer::remove_timer(timer_id);
        }
        if initial_expirations != 0 {
            self.inner.waiter.lock().wake_all(|event| event);
            self.inner.epoll_notifier.notify(FileEvent::READ_READY);
        }
        if let Some(expiry_us) = next_expiry_us {
            self.inner.arm_at(sequence, expiry_us);
        }

        Ok(old)
    }

    fn next_from_absolute(&self, value: Duration, interval: Duration) -> SysResult<TimerFdNext> {
        let clock_now = self.clock_id.now()?;
        let timer_now_us = now_us();

        if value > clock_now {
            return Ok(TimerFdNext::Armed {
                initial_expirations: 0,
                next_expiry_us: Some(timer_now_us.saturating_add(duration_to_us(value - clock_now))),
            });
        }

        if interval.is_zero() {
            return Ok(TimerFdNext::Armed {
                initial_expirations: 1,
                next_expiry_us: None,
            });
        }

        let interval_us = duration_to_us(interval);
        let elapsed_us = duration_to_us(clock_now - value);
        let missed = elapsed_us / interval_us + 1;
        let delay_us = interval_us - elapsed_us % interval_us;

        Ok(TimerFdNext::Armed {
            initial_expirations: missed,
            next_expiry_us: Some(timer_now_us.saturating_add(delay_us)),
        })
    }

    fn validate_io_len(len: usize) -> SysResult<()> {
        if len >= Self::IO_BYTES {
            Ok(())
        } else {
            Err(Errno::EINVAL)
        }
    }

    fn blocked(&self) -> bool {
        self.flags.lock().blocked
    }

    fn read_expirations(&self) -> SysResult<u64> {
        loop {
            let mut state = self.inner.state.lock();
            if state.expirations != 0 {
                let expirations = state.expirations;
                state.expirations = 0;
                return Ok(expirations);
            }

            if !self.blocked() {
                return Err(Errno::EAGAIN);
            }

            self.inner.waiter.lock().wait_current(Event::ReadReady);
            drop(state);

            current::schedule();
            match current::task().take_wakeup_event().unwrap() {
                Event::ReadReady => {}
                Event::Signal => {
                    self.inner.waiter.lock().remove(current::task());
                    return Err(Errno::EINTR);
                }
                event => unreachable!("unexpected event while waiting on timerfd read: {:?}", event),
            }
        }
    }
}

impl Drop for TimerFd {
    fn drop(&mut self) {
        self.inner.disarm();
    }
}

impl FileOps for TimerFd {
    fn read(&self, buf: &mut [u8]) -> SysResult<usize> {
        Self::validate_io_len(buf.len())?;
        let expirations = self.read_expirations()?;
        buf[..Self::IO_BYTES].copy_from_slice(&expirations.to_ne_bytes());
        Ok(Self::IO_BYTES)
    }

    fn read_to_user(&self, ubuf: &UAddrSpaceBuffer) -> SysResult<usize> {
        Self::validate_io_len(ubuf.length())?;
        let expirations = self.read_expirations()?;
        copy_to_user::buffer(ubuf.uaddr(), &expirations.to_ne_bytes())?;
        Ok(Self::IO_BYTES)
    }

    fn write(&self, _buf: &[u8]) -> SysResult<usize> {
        Err(Errno::EINVAL)
    }

    fn flags(&self) -> FileFlags {
        *self.flags.lock()
    }

    fn fstat(&self) -> SysResult<FileStat> {
        let mut kstat = FileStat::empty();
        kstat.st_ino = self as *const Self as *const () as u64;
        kstat.st_mode = (Mode::S_IFREG | Mode::S_IRUSR).bits();
        kstat.st_nlink = 1;
        Ok(kstat)
    }

    fn fsync(&self) -> SysResult<()> {
        Ok(())
    }

    fn get_inode(&self) -> Option<&Arc<dyn InodeOps>> {
        None
    }

    fn get_dentry(&self) -> Option<&Arc<Dentry>> {
        None
    }

    fn poll_event(&self, event: FileEvent) -> SysResult<Option<FileEvent>> {
        if !event.contains(FileEvent::READ_READY) {
            return Ok(None);
        }

        let state = self.inner.state.lock();
        if state.expirations != 0 {
            return Ok(Some(FileEvent::READ_READY));
        }

        Ok(None)
    }

    fn wait_event(&self, waker: usize, event: FileEvent) -> SysResult<Option<FileEvent>> {
        if !event.contains(FileEvent::READ_READY) {
            return Ok(None);
        }

        if let Some(ready) = self.poll_event(event)? {
            return Ok(Some(ready));
        }

        self.inner.waiter.lock().wait(
            current::task().clone(),
            Event::Poll {
                event: FileEvent::READ_READY,
                waker,
            },
        );

        Ok(None)
    }

    fn wait_event_cancel(&self) {
        self.inner.waiter.lock().remove(current::task());
    }

    fn epoll_notifier(&self) -> Option<Arc<EpollNotifier>> {
        Some(self.inner.epoll_notifier.clone())
    }

    fn set_flags(&self, flags: FileFlags) {
        *self.flags.lock() = FileFlags {
            readable: true,
            writable: false,
            blocked: flags.blocked,
            append: false,
            direct: false,
        };
    }

    fn type_name(&self) -> &'static str {
        "timerfd"
    }
}

enum TimerFdNext {
    Disarmed,
    Armed {
        initial_expirations: u64,
        next_expiry_us: Option<u64>,
    },
}

fn now_us() -> u64 {
    timer::now().as_micros() as u64
}

fn duration_to_us(duration: Duration) -> u64 {
    let micros = duration.as_micros();
    if micros == 0 && !duration.is_zero() {
        1
    } else {
        micros.min(u64::MAX as u128) as u64
    }
}
