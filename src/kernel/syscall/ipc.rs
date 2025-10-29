use alloc::sync::Arc;
use core::time::Duration;
use bitflags::bitflags;

use crate::kernel::config;
use crate::kernel::event::{timer, Event};
use crate::kernel::ipc::{Pipe, SignalSet};
use crate::kernel::scheduler::current;
use crate::kernel::syscall::uptr::{UArray, UPtr, UserPointer};
use crate::kernel::task::fdtable::FDFlags;
use crate::kernel::errno::Errno;
use crate::kernel::uapi::{self, Timespec};
use crate::kernel::task::manager;
use crate::{arch, kinfo};

use super::SyscallRet;

bitflags! {
    struct PipeFlags: usize {
        const O_CLOEXEC = 0x80000;
    }
}

pub fn pipe(uptr_pipefd: UArray<i32>, flags: usize) -> SyscallRet {
    let flags = PipeFlags::from_bits_truncate(flags);
    let fd_flags = FDFlags{
        cloexec: flags.contains(PipeFlags::O_CLOEXEC),
    };
    
    let (read_end, write_end) = Pipe::create(config::PIPE_CAPACITY);
    let read_end = Arc::new(read_end);
    let write_end = Arc::new(write_end);

    let (read_fd, write_fd);
    {
        let mut fdtable = current::fdtable().lock();
        read_fd = fdtable.push(read_end, fd_flags)?;
        write_fd = fdtable.push(write_end, fd_flags)?;
    }

    uptr_pipefd.write(0, &[read_fd as i32, write_fd as i32])?;

    Ok(0)
}

pub fn kill(pid: usize, signum: usize) -> SyscallRet {
    let pid = pid as i32;
    let signum = signum as u32;

    kinfo!("kill: pid={}, signum={}", pid, signum);
    
    if pid > 0 {
        let pcb = manager::get(pid).ok_or(Errno::ESRCH)?;
        pcb.send_signal(signum.try_into()?, current::tid(), None)?;
    }

    Ok(0)
}

pub fn rt_sigprocmask(_how: usize, uptr_set: UPtr<SignalSet>, uptr_oldset: UPtr<SignalSet>) -> SyscallRet {
    let mut signal_mask = current::tcb().signal_mask.lock();
    if !uptr_oldset.is_null() {
        let old_mask = signal_mask.clone();
        uptr_oldset.write(old_mask)?;
    }
    if !uptr_set.is_null() {
        let new_mask = uptr_set.read()?;
        *signal_mask = new_mask;
    }
    Ok(0)
}

pub fn rt_sigaction(signum: usize, uptr_act: UPtr<uapi::Sigaction>, uptr_oldact: UPtr<uapi::Sigaction>, sigsetsize: usize) -> SyscallRet {
    assert!(sigsetsize == core::mem::size_of::<SignalSet>());

    let signum = (signum as u32).try_into()?;

    let mut signal_actions = current::signal_actions().lock();
    if !uptr_oldact.is_null() {
        let old_action = signal_actions.get(signum);
        uptr_oldact.write(old_action.into())?;
    }

    if !uptr_act.is_null() {
        let new_action = uptr_act.read()?;
        signal_actions.set(signum, &new_action.into())?;
    }

    Ok(0)
}

pub fn rt_sig_return() -> SyscallRet {
    current::tcb().return_from_signal();
    arch::return_to_user();
}

pub fn sigtimedwait(uptr_set: UPtr<SignalSet>, _uptr_info: UPtr<()>, uptr_timeout: UPtr<Timespec>) -> SyscallRet {
    uptr_set.should_not_null()?;
    
    let timeout = uptr_timeout.read_optional()?;
    let signal_set = uptr_set.read()?;

    let mut state = current::tcb().state().lock();
    if let Some(pending) = state.pending_signal {
        if signal_set.contains(pending.signum) {
            state.pending_signal.take();
            return Ok(pending.signum.into());
        }
    }

    if let Some(ts) = timeout {
        let timeout_duration = Duration::new(ts.tv_sec as u64, ts.tv_nsec as u32);
        timer::add_timer(current::tcb().clone(), timeout_duration);
    }

    state.waiting_signal = signal_set;

    drop(state);

    current::block("sigtimedwait");

    match current::tcb().state().lock().event.take() {
        Some(Event::WaitSignal { signum }) => Ok(signum.into()),
        Some(Event::Signal) => Err(Errno::EINTR),
        Some(Event::Timeout) => Err(Errno::EAGAIN),
        _ => unreachable!(),
    }
}
