use alloc::boxed::Box;
use alloc::sync::Arc;
use alloc::vec;
use alloc::vec::Vec;
use bitflags::bitflags;
use core::convert::TryInto;
use core::time::Duration;
use num_enum::TryFromPrimitive;

use crate::arch;
use crate::fs::file::FileOps;
use crate::kernel::errno::Errno;
use crate::kernel::event::{Event, EventFd, FileEvent, TimerFd, TimerFdClockId, timer};
use crate::kernel::ipc::{KSiFields, SiCode, SignalNum, SignalSet, signum};
use crate::kernel::scheduler::{Task, current};
use crate::kernel::syscall::SysResult;
use crate::kernel::syscall::uptr::{UArray, UPtr, UserPointer, UserStruct};
use crate::kernel::task::PCB;
use crate::kernel::task::fdtable::FDFlags;
use crate::kernel::uapi::OpenFlags;
use crate::klib::defer;

use super::common::{ITimerSpec, ITimerVal, Timespec32};

const FD_SET_SIZE: usize = 1024;

bitflags! {
    struct EventFdFlags: usize {
        const EFD_SEMAPHORE = 1;
        const EFD_NONBLOCK = OpenFlags::O_NONBLOCK.bits();
        const EFD_CLOEXEC = OpenFlags::O_CLOEXEC.bits();
    }
}

bitflags! {
    struct TimerFdCreateFlags: usize {
        const TFD_NONBLOCK = OpenFlags::O_NONBLOCK.bits();
        const TFD_CLOEXEC = OpenFlags::O_CLOEXEC.bits();
    }
}

bitflags! {
    struct TimerFdSetTimeFlags: usize {
        const TFD_TIMER_ABSTIME = 1;
        const TFD_TIMER_CANCEL_ON_SET = 1 << 1;
    }
}

pub fn eventfd2(initval: usize, flags: usize) -> SysResult<usize> {
    let flags = EventFdFlags::from_bits(flags).ok_or(Errno::EINVAL)?;
    let eventfd = Arc::new(EventFd::new(
        initval as u32 as u64,
        !flags.contains(EventFdFlags::EFD_NONBLOCK),
        flags.contains(EventFdFlags::EFD_SEMAPHORE),
    ));
    let fd = current::fdtable().lock().push(
        eventfd,
        FDFlags {
            cloexec: flags.contains(EventFdFlags::EFD_CLOEXEC),
        },
    )?;
    Ok(fd)
}

pub fn timerfd_create(clockid: usize, flags: usize) -> SysResult<usize> {
    let clock_id = TimerFdClockId::try_from(clockid).map_err(|_| Errno::EINVAL)?;
    let flags = TimerFdCreateFlags::from_bits(flags).ok_or(Errno::EINVAL)?;
    let timerfd = Arc::new(TimerFd::new(
        clock_id,
        !flags.contains(TimerFdCreateFlags::TFD_NONBLOCK),
    ));
    let fd = current::fdtable().lock().push(
        timerfd,
        FDFlags {
            cloexec: flags.contains(TimerFdCreateFlags::TFD_CLOEXEC),
        },
    )?;
    Ok(fd)
}

pub fn timerfd_settime(
    fd: usize,
    flags: usize,
    uptr_new_value: UPtr<ITimerSpec>,
    uptr_old_value: UPtr<ITimerSpec>,
) -> SysResult<usize> {
    uptr_new_value.should_not_null()?;

    let flags = TimerFdSetTimeFlags::from_bits(flags).ok_or(Errno::EINVAL)?;
    let (value, interval) = uptr_new_value.read()?.into_durations()?;
    let file = current::fdtable().lock().get(fd)?;
    let timerfd = file.downcast_ref::<TimerFd>().ok_or(Errno::EINVAL)?;

    let old = timerfd.set_time(value, interval, flags.contains(TimerFdSetTimeFlags::TFD_TIMER_ABSTIME))?;
    if !uptr_old_value.is_null() {
        uptr_old_value.write(ITimerSpec::from_durations(old.interval, old.value))?;
    }

    Ok(0)
}

pub fn timerfd_gettime(fd: usize, uptr_value: UPtr<ITimerSpec>) -> SysResult<usize> {
    let file = current::fdtable().lock().get(fd)?;
    let timerfd = file.downcast_ref::<TimerFd>().ok_or(Errno::EINVAL)?;
    let time = timerfd.get_time();
    uptr_value.write(ITimerSpec::from_durations(time.interval, time.value))?;

    Ok(0)
}

#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct FdSet {
    fds_bits: [usize; FD_SET_SIZE / (8 * core::mem::size_of::<usize>())], // support up to 512 fds
}
impl UserStruct for FdSet {}

impl FdSet {
    fn bit_index(fd: usize) -> (usize, usize) {
        let bits = 8 * core::mem::size_of::<usize>();
        (fd / bits, fd % bits)
    }

    fn set(&mut self, fd: usize) {
        let (index, bit) = Self::bit_index(fd);
        self.fds_bits[index] |= 1usize << bit;
    }

    fn clear(&mut self, fd: usize) -> bool {
        let (index, bit) = Self::bit_index(fd);
        let t = (self.fds_bits[index] & (1usize << bit)) != 0;
        self.fds_bits[index] &= !(1usize << bit);
        t
    }
}

fn set_select_fd_by_event(
    event: FileEvent,
    fd: usize,
    readfds: &mut Option<FdSet>,
    writefds: &mut Option<FdSet>,
    exceptfds: &mut Option<FdSet>,
) {
    if event.intersects(FileEvent::READ_READY | FileEvent::HANG_UP) {
        readfds.as_mut().map(|set| set.set(fd));
    }
    if event.intersects(FileEvent::WRITE_READY | FileEvent::HANG_UP) {
        writefds.as_mut().map(|set| set.set(fd));
    }
    if event.contains(FileEvent::PRIORITY) {
        exceptfds.as_mut().map(|set| set.set(fd));
    }
}

fn write_back_fdsets(
    read_ptr: &UPtr<FdSet>,
    write_ptr: &UPtr<FdSet>,
    except_ptr: &UPtr<FdSet>,
    readfds: &Option<FdSet>,
    writefds: &Option<FdSet>,
    exceptfds: &Option<FdSet>,
) -> SysResult<()> {
    if let Some(t) = readfds {
        read_ptr.write(*t)?;
    }
    if let Some(t) = writefds {
        write_ptr.write(*t)?;
    }
    if let Some(t) = exceptfds {
        except_ptr.write(*t)?;
    }
    Ok(())
}

fn has_pending_unmasked_signal() -> bool {
    let tcb = current::tcb();
    tcb.recive_pending_signal_from_parent();
    tcb.state().lock().pending_signal.is_some()
}

fn select(
    nfds: usize,
    uptr_readfds: UPtr<FdSet>,
    uptr_writefds: UPtr<FdSet>,
    uptr_exceptfds: UPtr<FdSet>,
    timeout: Option<Duration>,
    uptr_sigmask: UPtr<SignalSet>,
) -> SysResult<usize> {
    let mut readfds = uptr_readfds.read_optional()?;
    let mut writefds = uptr_writefds.read_optional()?;
    let mut exceptfds = uptr_exceptfds.read_optional()?;
    let sigmask = uptr_sigmask.read_optional()?;

    let mut files_to_select = Vec::new();
    let fdtable = current::fdtable();
    let mut fdtable = fdtable.lock();

    for i in 0..nfds {
        let want_read = readfds.as_mut().map_or(false, |set| set.clear(i));
        let want_write = writefds.as_mut().map_or(false, |set| set.clear(i));
        let want_except = exceptfds.as_mut().map_or(false, |set| set.clear(i));

        if !want_read && !want_write && !want_except {
            continue;
        }

        let mut wait_set = FileEvent::empty();
        if want_read {
            wait_set |= FileEvent::READ_READY;
        }
        if want_write {
            wait_set |= FileEvent::WRITE_READY;
        }
        if want_except {
            wait_set |= FileEvent::PRIORITY;
        }

        let file = fdtable.get(i).map_err(|_| Errno::EBADF)?;
        files_to_select.push((i, file, wait_set));
    }
    drop(fdtable);

    let tcb = current::tcb();
    tcb.block("select");
    let defer = defer::defer(|| {
        tcb.unblock();
    });

    let mut ready_count = 0;
    let mut waiting_files = Vec::new();
    let r = files_to_select
        .iter()
        .enumerate()
        .try_for_each(|(i, (fd, file, wait_set))| {
            if let Some(event) = file.wait_event(i, *wait_set)? {
                set_select_fd_by_event(event, *fd, &mut readfds, &mut writefds, &mut exceptfds);
                ready_count += 1;
            } else {
                waiting_files.push(file);
            }
            Ok(())
        });

    if let Err(e) = r {
        // Clean up waiters on error
        for file in files_to_select.iter().map(|(_, f, _)| f) {
            file.wait_event_cancel();
        }
        return Err(e);
    }

    if ready_count > 0 {
        // Clean up waiters
        for file in files_to_select.iter().map(|(_, f, _)| f) {
            file.wait_event_cancel();
        }

        write_back_fdsets(
            &uptr_readfds,
            &uptr_writefds,
            &uptr_exceptfds,
            &readfds,
            &writefds,
            &exceptfds,
        )?;
        return Ok(ready_count);
    }

    let timer_id = if let Some(duration) = timeout {
        if !duration.is_zero() {
            Some(timer::add_timer(current::task().clone(), duration))
        } else {
            // timeout is zero, return immediately
            for file in files_to_select.iter().map(|(_, f, _)| f) {
                file.wait_event_cancel();
            }
            write_back_fdsets(
                &uptr_readfds,
                &uptr_writefds,
                &uptr_exceptfds,
                &readfds,
                &writefds,
                &exceptfds,
            )?;

            return Ok(0);
        }
    } else {
        None
    };

    let old_signal_mask = sigmask.map(|mask| tcb.swap_signal_mask(mask));

    if has_pending_unmasked_signal() {
        old_signal_mask.map(|mask| tcb.set_signal_mask(mask));
        waiting_files.iter().for_each(|file| {
            file.wait_event_cancel();
        });
        timer_id.map(|id| timer::remove_timer(id));

        write_back_fdsets(
            &uptr_readfds,
            &uptr_writefds,
            &uptr_exceptfds,
            &readfds,
            &writefds,
            &exceptfds,
        )?;

        return Err(Errno::EINTR);
    }

    defer::cancel(defer);
    current::schedule();

    old_signal_mask.map(|mask| {
        tcb.set_signal_mask(mask);
    });
    // tcb.state().lock().state = TaskState::Running;

    let event = tcb.take_wakeup_event().unwrap();

    match event {
        Event::Poll { event, waker } => {
            debug_assert!(waker < files_to_select.len());
            waiting_files.iter().enumerate().for_each(|(i, file)| {
                if i != waker {
                    file.wait_event_cancel();
                }
            });

            set_select_fd_by_event(
                event,
                files_to_select[waker].0,
                &mut readfds,
                &mut writefds,
                &mut exceptfds,
            );
            timer_id.map(|id| timer::remove_timer(id));

            write_back_fdsets(
                &uptr_readfds,
                &uptr_writefds,
                &uptr_exceptfds,
                &readfds,
                &writefds,
                &exceptfds,
            )?;

            Ok(1)
        }
        Event::Timeout => {
            waiting_files.iter().for_each(|file| {
                file.wait_event_cancel();
            });

            write_back_fdsets(
                &uptr_readfds,
                &uptr_writefds,
                &uptr_exceptfds,
                &readfds,
                &writefds,
                &exceptfds,
            )?;

            Ok(0)
        }
        Event::Signal => {
            waiting_files.iter().for_each(|file| {
                file.wait_event_cancel();
            });
            timer_id.map(|id| timer::remove_timer(id));

            write_back_fdsets(
                &uptr_readfds,
                &uptr_writefds,
                &uptr_exceptfds,
                &readfds,
                &writefds,
                &exceptfds,
            )?;

            Err(Errno::EINTR)
        }
        _ => unreachable!("Invalid event type in select: {:?}", event),
    }
}

pub fn pselect6_time32(
    nfds: usize,
    uptr_readfds: UPtr<FdSet>,
    uptr_writefds: UPtr<FdSet>,
    uptr_exceptfds: UPtr<FdSet>,
    uptr_timeout: UPtr<Timespec32>,
    uptr_sigmask: UPtr<SignalSet>,
) -> SysResult<usize> {
    if nfds > FD_SET_SIZE {
        return Err(Errno::EINVAL);
    }

    let timeout: Option<Duration> = match uptr_timeout.read_optional()? {
        Some(ts) => Some(ts.try_into()?),
        None => None,
    };

    select(nfds, uptr_readfds, uptr_writefds, uptr_exceptfds, timeout, uptr_sigmask)
}

bitflags! {
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    struct PollEventSet: i16 {
        const POLLIN   = FileEvent::READ_READY.bits();  // There is data to read.
        const POLLPRI  = FileEvent::PRIORITY.bits();    // There is urgent data to read.
        const POLLOUT  = FileEvent::WRITE_READY.bits(); // Writing now will not block.
        const POLLERR  = FileEvent::ERROR.bits();       // Error condition.
        const POLLHUP  = FileEvent::HANG_UP.bits();     // Hung up.
        const POLLNVAL = FileEvent::INVALID.bits();     // Invalid request: fd not open.
    }
}

impl Into<FileEvent> for PollEventSet {
    fn into(self) -> FileEvent {
        FileEvent::from_bits_truncate(self.bits())
    }
}

impl From<FileEvent> for PollEventSet {
    fn from(event: FileEvent) -> Self {
        PollEventSet::from_bits_truncate(event.bits())
    }
}

#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct Pollfd {
    fd: i32,
    events: PollEventSet,
    revents: PollEventSet,
}
impl UserStruct for Pollfd {}

impl Pollfd {
    pub fn default() -> Self {
        Self {
            fd: -1,
            events: PollEventSet::empty(),
            revents: PollEventSet::empty(),
        }
    }
}

fn do_poll(pollfds: &mut [Pollfd], timeout: Option<Duration>, sigmask: Option<SignalSet>) -> SysResult<usize> {
    let fdtable = current::fdtable();
    let mut fdtable = fdtable.lock();

    pollfds.iter_mut().for_each(|pfd| {
        pfd.revents = PollEventSet::empty();
    });

    let mut count = 0u32;
    let mut i = 0;

    let tcb = current::tcb();
    tcb.block("poll");
    let defer = defer::defer(|| {
        tcb.unblock();
    });

    let mut poll_files: Vec<(Arc<dyn FileOps>, &mut Pollfd)> = pollfds
        .iter_mut()
        .filter_map(|pfd| {
            if pfd.fd < 0 {
                count += 1;
                return None;
            }

            let file = match fdtable.get(pfd.fd as usize) {
                Ok(f) => f,
                Err(_) => {
                    pfd.revents = PollEventSet::POLLNVAL;
                    count += 1;
                    return None;
                }
            };

            let wait_set = pfd.events.into();
            if let Some(event) = file.wait_event(i, wait_set).unwrap() {
                pfd.revents = event.into();
                count += 1;
            }

            i += 1;
            Some((file, pfd))
        })
        .collect();

    drop(fdtable);

    if count != 0 {
        poll_files.iter_mut().for_each(|(file, _)| {
            file.wait_event_cancel();
        });
        return Ok(count as usize);
    }

    let timer_id = match timeout {
        Some(timeout) if timeout.is_zero() => {
            poll_files.iter_mut().for_each(|(file, _)| {
                file.wait_event_cancel();
            });
            return Ok(0);
        }
        Some(timeout) => Some(timer::add_timer(current::task().clone(), timeout)),
        None => None,
    };

    let tcb = current::tcb();
    let old_signal_mask = sigmask.map(|mask| tcb.swap_signal_mask(mask));

    if has_pending_unmasked_signal() {
        old_signal_mask.map(|mask| tcb.set_signal_mask(mask));
        timer_id.map(|id| timer::remove_timer(id));
        poll_files.iter_mut().for_each(|(file, _)| {
            file.wait_event_cancel();
        });
        return Err(Errno::EINTR);
    }

    defer::cancel(defer);
    current::schedule();

    old_signal_mask.map(|mask| tcb.set_signal_mask(mask));

    // start polling
    let event = current::task().take_wakeup_event().unwrap();

    let mut cancel_all = || {
        poll_files.iter_mut().for_each(|(file, _)| {
            file.wait_event_cancel();
        });
    };

    let (poll_event, waker) = match event {
        Event::Poll { event, waker } => (event, waker),
        Event::Timeout => {
            timer_id.map(|id| timer::remove_timer(id));
            cancel_all();
            return Ok(0);
        }
        Event::Signal => {
            timer_id.map(|id| timer::remove_timer(id));
            cancel_all();
            return Err(Errno::EINTR);
        }
        _ => unreachable!("Invalid event type in poll: {:?}", event),
    };

    debug_assert!(waker < poll_files.len());

    poll_files.iter_mut().enumerate().for_each(|(i, (file, pfd))| {
        if i != waker {
            file.wait_event_cancel();
            pfd.revents = PollEventSet::empty();
        }
    });

    poll_files[waker].1.revents = poll_event.into();

    timer_id.map(|id| timer::remove_timer(id));

    Ok(1)
}

pub fn ppoll_time32(
    uptr_ufds: UArray<Pollfd>,
    nfds: usize,
    uptr_timeout: UPtr<Timespec32>,
    uptr_sigmask: UPtr<SignalSet>,
    sigmask_size: usize,
) -> SysResult<usize> {
    if nfds > 0 {
        uptr_ufds.should_not_null()?;
    }

    if nfds > current::fdtable().lock().get_max_fd() {
        return Err(Errno::EINVAL);
    }

    let mut pollfds = vec![Pollfd::default(); nfds];
    if nfds > 0 {
        uptr_ufds.read(0, &mut pollfds)?;
    }

    let timeout = if !uptr_timeout.is_null() {
        Some(uptr_timeout.read()?.try_into()?)
    } else {
        None
    };

    if !uptr_sigmask.is_null() && sigmask_size != core::mem::size_of::<SignalSet>() {
        return Err(Errno::EINVAL);
    }

    let sigmask = uptr_sigmask.read_optional()?;

    let r = do_poll(&mut pollfds, timeout, sigmask)?;

    if nfds > 0 {
        uptr_ufds.write(0, &pollfds)?;
    }

    Ok(r)
}

#[repr(usize)]
#[derive(Debug, TryFromPrimitive)]
enum ITimerWhich {
    Real,
    Virtual,
    Prof,
}

fn setitimer_helper(signum: SignalNum, interval: Duration, pcb: Arc<PCB>, which: usize) {
    if pcb.is_exited() {
        return;
    }

    // Check if this timer is still active (not cancelled)
    if pcb.itimer_ids.lock()[which].is_none() {
        return;
    }

    let _ = pcb.send_signal(signum, SiCode::SI_KERNEL, 0, KSiFields::Empty, None);

    // Schedule next interval
    let expiry_us = arch::get_time_us() + interval.as_micros() as u64;
    pcb.itimer_expiry_us.lock()[which] = expiry_us;

    let pcb_clone = pcb.clone();
    let timer_id = timer::add_timer_with_callback(
        interval,
        Box::new(move || {
            setitimer_helper(signum, interval, pcb_clone, which);
        }),
    );
    pcb.itimer_ids.lock()[which] = Some(timer_id);
}

/// Compute remaining time for an itimer. Returns (remaining value, interval).
fn get_old_itimer(pcb: &Arc<PCB>, which: usize) -> ITimerVal {
    let expiry_us = pcb.itimer_expiry_us.lock()[which];
    let interval = pcb.itimer_interval.lock()[which];

    let remaining = if expiry_us == 0 {
        Duration::ZERO
    } else {
        let now = arch::get_time_us();
        if now >= expiry_us {
            Duration::ZERO
        } else {
            let rem_us = expiry_us - now;
            Duration::from_micros(rem_us)
        }
    };

    ITimerVal::from_durations(interval, remaining)
}

pub fn getitimer(which: usize, uptr_value: UPtr<ITimerVal>) -> SysResult<usize> {
    ITimerWhich::try_from(which).map_err(|_| Errno::EINVAL)?;
    uptr_value.should_not_null()?;

    let value = get_old_itimer(&current::pcb(), which);
    uptr_value.write(value)?;

    Ok(0)
}

pub fn setitimer(which: usize, uptr_new_value: UPtr<ITimerVal>, uptr_old_value: UPtr<ITimerVal>) -> SysResult<usize> {
    let which_enum = ITimerWhich::try_from(which).map_err(|_| Errno::EINVAL)?;
    let pcb = current::pcb();

    // Write old value if requested
    if !uptr_old_value.is_null() {
        let old = get_old_itimer(&pcb, which);
        uptr_old_value.write(old)?;
    }

    // If new_value is null, this is just a query (getitimer behavior)
    if uptr_new_value.is_null() {
        return Ok(0);
    }

    let new_value = uptr_new_value.read()?;

    // Cancel existing timer
    {
        let mut ids = pcb.itimer_ids.lock();
        if let Some(id) = ids[which] {
            timer::remove_timer(id);
        }
        ids[which] = None;
    }
    pcb.itimer_expiry_us.lock()[which] = 0;
    pcb.itimer_interval.lock()[which] = Duration::ZERO;

    if new_value.it_value.is_zero() {
        return Ok(0);
    }

    let signum = match which_enum {
        ITimerWhich::Real => signum::SIGALRM,
        ITimerWhich::Virtual => signum::SIGVTALRM,
        ITimerWhich::Prof => signum::SIGPROF,
    };

    let interval = new_value.it_interval;

    if interval.tv_usec >= 1_000_000 {
        return Err(Errno::EINVAL);
    }

    let value_dur: Duration = new_value.it_value.into();
    let expiry_us = arch::get_time_us() + value_dur.as_micros() as u64;
    pcb.itimer_expiry_us.lock()[which] = expiry_us;
    pcb.itimer_interval.lock()[which] = interval.into();

    let timer_id = if interval.is_zero() {
        let pcb_cb = pcb.clone();
        timer::add_timer_with_callback(
            value_dur,
            Box::new(move || {
                let _ = pcb_cb.send_signal(signum, SiCode::SI_KERNEL, 0, KSiFields::Empty, None);
                pcb_cb.itimer_ids.lock()[which] = None;
                pcb_cb.itimer_expiry_us.lock()[which] = 0;
            }),
        )
    } else {
        let interval_dur: Duration = interval.into();
        let pcb_cb = pcb.clone();
        timer::add_timer_with_callback(
            value_dur,
            Box::new(move || {
                setitimer_helper(signum, interval_dur, pcb_cb, which);
            }),
        )
    };

    pcb.itimer_ids.lock()[which] = Some(timer_id);

    Ok(0)
}
