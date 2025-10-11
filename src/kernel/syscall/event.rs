use alloc::vec;

use crate::kernel::event::{Event, PollEvent, PollEventSet};
use crate::kernel::scheduler::current;
use crate::kernel::syscall::SysResult;
use crate::kernel::errno::Errno;
use crate::{copy_from_user, copy_to_user_ref};

#[repr(C)]
#[derive(Clone, Copy, Debug)]
struct Pollfd {
    pub fd:     i32,
    pub events: i16,
    pub revents: i16,
}

impl Pollfd {
    pub fn default() -> Self {
        Self { fd: -1, events: -1, revents: -1 }
    }
}

#[repr(C)]
struct Timespec32 {
    pub tv_sec:  i32,     // seconds
    pub tv_nsec: i32,     // nanoseconds
}

fn poll(pollfds: &mut [Pollfd], _timeout: Option<u64>) -> SysResult<usize> {
    let mut fdtable = current::fdtable().lock();

    let mut count = 0u32;
    for (i, pfd) in pollfds.iter_mut().enumerate() {
        if pfd.fd < 0 {
            count += 1;
            continue;
        }

        // kinfo!("Poll: fd={}, events={:x}", pfd.fd, pfd.events);
            
        let file = match fdtable.get(pfd.fd as usize) {
            Ok(f) => f,
            Err(_) => {
                pfd.revents = PollEventSet::POLLNVAL.bits();
                count += 1; 
                continue;
            }
        };
            
        let poll_set = PollEventSet::from_bits_truncate(pfd.events);
        if let Some(event) = file.poll(i, poll_set)? {
            pfd.revents = match event {
                PollEvent::ReadReady  => PollEventSet::POLLIN.bits(),
                PollEvent::WriteReady => PollEventSet::POLLOUT.bits(),
                PollEvent::Priority   => PollEventSet::POLLPRI.bits(),
                PollEvent::HangUp     => PollEventSet::POLLHUP.bits(),
                _ => unreachable!(),
            };
            count += 1;
        }
    }

    if count != 0 {
        return Ok(count as usize);
    }
    
    // start polling
    current::tcb().block("poll");
    current::schedule();

    let event = current::tcb().state().lock().event.unwrap();

    let (poll_event, waker) = match event.event {
        Event::Poll(event, waker) => (event, waker),
        Event::Timeout => return Ok(0), // Timeout occurred
        _ => return Err(Errno::EINTR),  // Interrupted by other events
    };

    assert!(waker < pollfds.len());
    let pfd = &mut pollfds[waker];
    pfd.revents = match poll_event {
        PollEvent::ReadReady  => PollEventSet::POLLIN.bits(),
        PollEvent::WriteReady => PollEventSet::POLLOUT.bits(),
        PollEvent::Priority   => PollEventSet::POLLPRI.bits(),
        PollEvent::HangUp     => PollEventSet::POLLHUP.bits(),
    };

    Ok(1)
}

pub fn ppoll_time32(uptr_ufds: usize, nfds: usize, uptr_timeout: usize, _uptr_sigmask: usize, _sigmask_size: usize) -> SysResult<usize> {
    let mut pollfds = vec![Pollfd::default(); nfds];

    pollfds.iter_mut().enumerate().for_each(|(i, pfd)| {
        copy_from_user!(uptr_ufds + i * core::mem::size_of::<Pollfd>(), *pfd).unwrap();
    });

    let timeout = if uptr_timeout != 0 {
        let ts: Timespec32 = Timespec32 { tv_sec: 0, tv_nsec: 0 };
        copy_from_user!(uptr_timeout, ts)?;
        if ts.tv_sec < 0 || ts.tv_nsec < 0 || ts.tv_nsec >= 1_000_000_000 {
            return Err(Errno::EINVAL);
        }
        Some(ts.tv_sec as u64 * 1_000_000 + ts.tv_nsec as u64 / 1_000)
    } else {
        None
    };

    let r = poll(&mut pollfds, timeout)?;

    copy_to_user_ref!(uptr_ufds, pollfds.as_slice())?;

    Ok(r)
}
