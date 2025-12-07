use core::time::Duration;
use alloc::boxed::Box;
use alloc::vec;
use alloc::vec::Vec;
use alloc::sync::Arc;
use num_enum::TryFromPrimitive;

use crate::fs::file::FileOps;
use crate::kernel::event::{Event, FileEvent, PollEventSet, timer};
use crate::kernel::ipc::{KSiFields, SiCode, SignalSet, SignalNum, signum};
use crate::kernel::scheduler::{Task, TaskState, current};
use crate::kernel::syscall::uptr::{UArray, UPtr, UserPointer, UserStruct};
use crate::kernel::syscall::SysResult;
use crate::kernel::errno::Errno;
use crate::kernel::task::PCB;
use crate::kernel::uapi;
use crate::klib::utils::defer;

const FD_SET_SIZE: usize = 1024;

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
    match event {
        FileEvent::ReadReady => {
            readfds.as_mut().map(|set| set.set(fd));
        }
        FileEvent::WriteReady => {
            writefds.as_mut().map(|set| set.set(fd));
        }
        FileEvent::Priority => {
            exceptfds.as_mut().map(|set| set.set(fd));
        }
        FileEvent::HangUp => {
            readfds.as_mut().map(|set| set.set(fd));
            writefds.as_mut().map(|set| set.set(fd));
        }
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
    readfds.map(|t| read_ptr.write(t));
    writefds.map(|t| write_ptr.write(t));
    exceptfds.map(|t| except_ptr.write(t));
    Ok(())
}

fn select(
    nfds: usize,
    uptr_readfds: UPtr<FdSet>,
    uptr_writefds: UPtr<FdSet>,
    uptr_exceptfds: UPtr<FdSet>,
    timeout: Option<Duration>,
    uptr_sigmask: UPtr<SignalSet>
) -> SysResult<usize> {
    let mut readfds = uptr_readfds.read_optional()?;
    let mut writefds = uptr_writefds.read_optional()?;
    let mut exceptfds = uptr_exceptfds.read_optional()?;
    let sigmask = uptr_sigmask.read_optional()?;
    
    let mut files_to_select = Vec::new();
    let mut fdtable = current::fdtable().lock();
    
    for i in 0..nfds {
        let want_read = readfds.as_mut().map_or(false, |set| set.clear(i));
        let want_write = writefds.as_mut().map_or(false, |set| set.clear(i));
        let want_except = exceptfds.as_mut().map_or(false, |set| set.clear(i));

        if !want_read && !want_write && !want_except {
            continue;
        }

        let mut wait_set = PollEventSet::empty();
        if want_read {
            wait_set |= PollEventSet::POLLIN;
        }
        if want_write {
            wait_set |= PollEventSet::POLLOUT;
        }
        if want_except {
            wait_set |= PollEventSet::POLLPRI;
        }

        let file = fdtable.get(i).map_err(|_| Errno::EBADF)?;
        files_to_select.push((i, file, wait_set));
    }
    drop(fdtable);

    let tcb = current::tcb();
    tcb.block("select");

    let _d = defer(|| { tcb.state().lock().state = TaskState::Running; });
    
    let mut ready_count = 0;
    let mut waiting_files = Vec::new();
    let r = files_to_select.iter().enumerate().try_for_each(|(i, (fd, file, wait_set))| {
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
        
        write_back_fdsets(&uptr_readfds, &uptr_writefds, &uptr_exceptfds, &readfds, &writefds, &exceptfds)?;
        return Ok(ready_count);
    }

    if waiting_files.is_empty() {
        return Ok(0);
    }

    let timer_id = if let Some(duration) = timeout {
        if !duration.is_zero() {
            Some(timer::add_timer(current::task().clone(), duration))
        } else {
            // timeout is zero, return immediately
            for file in files_to_select.iter().map(|(_, f, _)| f) {
                file.wait_event_cancel();
            }
            write_back_fdsets(&uptr_readfds, &uptr_writefds, &uptr_exceptfds, &readfds, &writefds, &exceptfds)?;
            
            // Set task state back to running
            // tcb.state().lock().state = TaskState::Running;
            
            return Ok(0);
        }
    } else {
        None
    };

    let old_signal_mask = sigmask.map(|mask| tcb.swap_signal_mask(mask));

    current::schedule();

    old_signal_mask.map(|mask| {
        tcb.set_signal_mask(mask);
    });
    tcb.state().lock().state = TaskState::Running;

    let event = tcb.take_wakeup_event().unwrap();

    write_back_fdsets(&uptr_readfds, &uptr_writefds, &uptr_exceptfds, &readfds, &writefds, &exceptfds)?;
    
    match event {
        Event::Poll { event, waker } => {
            waiting_files.iter().enumerate().for_each(|(i, file)| {
                if i != waker {
                    file.wait_event_cancel();
                }
            });
            
            set_select_fd_by_event(event, waker, &mut readfds, &mut writefds, &mut exceptfds);
            timer_id.map(|id| timer::remove_timer(id));

            Ok(1)
        },
        Event::Timeout => Err(Errno::ETIMEDOUT),
        Event::Signal => {
            timer_id.map(|id| timer::remove_timer(id));
            Err(Errno::EINTR)
        }, 
        _ => unreachable!("Invalid event type in select: {:?}", event),
    }
}

pub fn pselect6_time32(
    nfds: usize,
    uptr_readfds: UPtr<FdSet>,
    uptr_writefds: UPtr<FdSet>,
    uptr_exceptfds: UPtr<FdSet>,
    uptr_timeout: UPtr<uapi::Timespec32>,
    uptr_sigmask: UPtr<SignalSet>,
) -> SysResult<usize> {
    if nfds == 0 || nfds > FD_SET_SIZE {
        return Err(Errno::EINVAL);
    }

    let timeout: Option<Duration> = uptr_timeout.read_optional()?.map(|ts| ts.into());

    select(
        nfds,
        uptr_readfds,
        uptr_writefds,
        uptr_exceptfds,
        timeout,
        uptr_sigmask,
    )
}

#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct Pollfd {
    pub fd:     i32,
    pub events: i16,
    pub revents: i16,
}
impl UserStruct for Pollfd {}

impl Pollfd {
    pub fn default() -> Self {
        Self { fd: -1, events: -1, revents: -1 }
    }
}

// BUG
fn poll(pollfds: &mut [Pollfd], timeout: Option<Duration>) -> SysResult<usize> {
    let mut fdtable = current::fdtable().lock();

    let mut count = 0u32;
    let mut i = 0;

    let mut poll_files: Vec<(Arc<dyn FileOps>, &mut Pollfd)> = pollfds.iter_mut()
        .filter_map(|pfd| {
            if pfd.fd < 0 {
                count += 1;
                return None;
            }

            let file = match fdtable.get(pfd.fd as usize) {
                Ok(f) => f,
                Err(_) => {
                    count += 1; 
                    return None;
                }
            };

            let poll_set = PollEventSet::from_bits_truncate(pfd.events);
            if let Some(event) = file.wait_event(i, poll_set).unwrap() {
                pfd.revents = match event {
                    FileEvent::ReadReady  => PollEventSet::POLLIN.bits(),
                    FileEvent::WriteReady => PollEventSet::POLLOUT.bits(),
                    FileEvent::Priority   => PollEventSet::POLLPRI.bits(),
                    FileEvent::HangUp     => PollEventSet::POLLHUP.bits(),
                };
                count += 1;
            }

            i += 1;
            Some((file, pfd))
        })
        .collect();

    drop(fdtable);

    if count != 0 {
        return Ok(count as usize);
    }

    if let Some(timeout) = timeout {
        timer::add_timer(current::task().clone(), timeout);
    }
    
    // start polling
    let event = current::block("poll");

    let (poll_event, waker) = match event {
        Event::Poll{ event, waker} => (event, waker),
        Event::Timeout => return Ok(0), // Timeout occurred
        Event::Signal => return Err(Errno::EINTR),  // Interrupted by other events
        _ => unreachable!("Invalid event type in poll: {:?}", event),
    };

    debug_assert!(waker < poll_files.len());

    poll_files.iter_mut().enumerate().for_each(|(i, (file, pfd))| {
        if i != waker {
            file.wait_event_cancel();
            pfd.revents = 0;
        }
    });

    poll_files[waker].1.revents = match poll_event {
        FileEvent::ReadReady  => PollEventSet::POLLIN.bits(),
        FileEvent::WriteReady => PollEventSet::POLLOUT.bits(),
        FileEvent::Priority   => PollEventSet::POLLPRI.bits(),
        FileEvent::HangUp     => PollEventSet::POLLHUP.bits(),
    };
    
    Ok(1)
}

pub fn ppoll_time32(uptr_ufds: UArray<Pollfd>, nfds: usize, uptr_timeout: UPtr<uapi::Timespec32>, _uptr_sigmask: usize, _sigmask_size: usize) -> SysResult<usize> {
    if nfds == 0 {
        return Ok(0);
    }

    uptr_ufds.should_not_null()?;
    
    let mut pollfds = vec![Pollfd::default(); nfds];
    uptr_ufds.read(0, &mut pollfds)?;

    let timeout = if !uptr_timeout.is_null() {
        let ts = uptr_timeout.read()?;
        if ts.tv_sec < 0 || ts.tv_nsec < 0 || ts.tv_nsec >= 1_000_000_000 {
            return Err(Errno::EINVAL);
        }
        Some(ts.into())
    } else {
        None
    };

    let r = poll(&mut pollfds, timeout)?;
    
    uptr_ufds.write(0, &pollfds)?;

    Ok(r)
}


// TODO: implement the setitimer syscall
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct ITimerValue {
    pub it_interval: uapi::TimeVal,
    pub it_value:    uapi::TimeVal,
}

impl UserStruct for ITimerValue {}

#[repr(usize)]
#[derive(Debug, TryFromPrimitive)]
enum ITimerWhich {
    Real,
    Virtual,
    Prof,
}

fn setitimer_helper(signum: SignalNum, time: Duration, pcb: Arc<PCB>, which: usize) {
    if pcb.is_exited() {
        return;
    }
    
    // Check if this timer is still active (not cancelled)
    if pcb.itimer_ids.lock()[which].is_none() {
        return;
    }

    let _ = pcb.send_signal(signum, SiCode::SI_KERNEL, KSiFields::Empty, None);
    let pcb_clone = pcb.clone();
    let timer_id = timer::add_timer_with_callback(time, Box::new(move || {
        setitimer_helper(signum, time, pcb_clone, which);
    }));
    pcb.itimer_ids.lock()[which] = Some(timer_id);
}

pub fn setitimer(which: usize, uptr_new_value: UPtr<ITimerValue>, _uptr_old_value: UPtr<ITimerValue>) -> SysResult<usize> {
    uptr_new_value.should_not_null()?;
    
    let which_enum = ITimerWhich::try_from(which).map_err(|_| Errno::EINVAL)?;
    let new_value = uptr_new_value.read()?;
    let pcb = current::pcb();

    // Cancel existing timer
    {
        let mut ids = pcb.itimer_ids.lock();
        if let Some(id) = ids[which] {
            timer::remove_timer(id);
        }
        ids[which] = None;
    }

    if new_value.it_value.is_zero() {
        return Ok(0);
    }

    let signum = match which_enum {
        ITimerWhich::Real    => signum::SIGALRM,
        ITimerWhich::Virtual => signum::SIGVTALRM,
        ITimerWhich::Prof    => signum::SIGPROF,
    };

    let pcb_clone = pcb.clone();
    let interval = new_value.it_interval;
    
    let timer_id = if interval.is_zero() {
        timer::add_timer_with_callback(new_value.it_value.into(), Box::new(move || {
            let _ = pcb_clone.send_signal(signum, SiCode::SI_KERNEL, KSiFields::Empty, None);
            pcb_clone.itimer_ids.lock()[which] = None;
        }))
    } else {
        timer::add_timer_with_callback(new_value.it_value.into(), Box::new(move || {
            setitimer_helper(signum, interval.into(), pcb_clone, which);
        }))
    };
    
    pcb.itimer_ids.lock()[which] = Some(timer_id);
    
    Ok(0)
}
