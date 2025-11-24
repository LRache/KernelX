use core::time::Duration;
use alloc::vec;
use alloc::vec::Vec;
use alloc::sync::Arc;

use crate::fs::file::FileOps;
use crate::kernel::event::{Event, PollEvent, PollEventSet, timer};
use crate::kernel::ipc::SignalSet;
use crate::kernel::scheduler::current;
use crate::kernel::syscall::uptr::{UArray, UPtr};
use crate::kernel::syscall::SysResult;
use crate::kernel::errno::Errno;
use crate::kernel::uapi::Timespec32;

const FD_SET_SIZE: usize = 1024;

#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct FdSet {
    fds_bits: [usize; FD_SET_SIZE / (8 * core::mem::size_of::<usize>())], // support up to 512 fds
}

pub fn pselect6_time32(nfds: usize, uptr_readfds: UPtr<FdSet>, uptr_writefds: UPtr<FdSet>, uptr_exceptfds: UPtr<FdSet>, uptr_timeout: UPtr<Timespec32>, _uptr_sigmask: UPtr<SignalSet>) -> SysResult<usize> {
    if nfds == 0 || nfds > FD_SET_SIZE {
        return Err(Errno::EINVAL);
    }

    let mut readfds = uptr_readfds.read_optional()?;
    let mut writefds = uptr_writefds.read_optional()?;
    let mut exceptfds = uptr_exceptfds.read_optional()?;

    let timeout: Option<Duration> = uptr_timeout.read_optional()?.map(|ts| {
        ts.into()
    });



    Ok(0)
}

#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct Pollfd {
    pub fd:     i32,
    pub events: i16,
    pub revents: i16,
}

impl Pollfd {
    pub fn default() -> Self {
        Self { fd: -1, events: -1, revents: -1 }
    }
}

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
            if let Some(event) = file.poll(i, poll_set).unwrap() {
                pfd.revents = match event {
                    PollEvent::ReadReady  => PollEventSet::POLLIN.bits(),
                    PollEvent::WriteReady => PollEventSet::POLLOUT.bits(),
                    PollEvent::Priority   => PollEventSet::POLLPRI.bits(),
                    PollEvent::HangUp     => PollEventSet::POLLHUP.bits(),
                };
                count += 1;
            }
            // kinfo!("poll: fd={}, events={:#x}, revents={:#x}", pfd.fd, pfd.events, pfd.revents);

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
            file.poll_cancel();
            pfd.revents = 0;
        }
    });

    poll_files[waker].1.revents = match poll_event {
        PollEvent::ReadReady  => PollEventSet::POLLIN.bits(),
        PollEvent::WriteReady => PollEventSet::POLLOUT.bits(),
        PollEvent::Priority   => PollEventSet::POLLPRI.bits(),
        PollEvent::HangUp     => PollEventSet::POLLHUP.bits(),
    };
    
    Ok(1)
}

pub fn ppoll_time32(uptr_ufds: UArray<Pollfd>, nfds: usize, uptr_timeout: UPtr<Timespec32>, _uptr_sigmask: usize, _sigmask_size: usize) -> SysResult<usize> {
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
