use alloc::sync::Arc;
use bitflags::bitflags;

use crate::kernel::config;
use crate::kernel::ipc::{Pipe, SignalSet};
use crate::kernel::scheduler::current;
use crate::kernel::syscall::uptr::{UArray, UPtr, UserPointer};
use crate::kernel::task::fdtable::FDFlags;
use crate::kernel::errno::Errno;
use crate::kernel::uapi;
use crate::kernel::task::manager;
use crate::arch;

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
    
    if pid > 0 {
        let pcb = manager::get(pid).ok_or(Errno::ESRCH)?;
        pcb.send_signal(signum, current::tid(), None)?;
    }

    Ok(0)
}

pub fn rt_sigprocmask(_how: usize, _set: usize, _oldset: usize) -> SyscallRet {
    Ok(0)
}

pub fn rt_sigaction(signum: usize, uptr_act: UPtr<uapi::Sigaction>, uptr_oldact: UPtr<uapi::Sigaction>, sigsetsize: usize) -> SyscallRet {
    assert!(sigsetsize == core::mem::size_of::<SignalSet>());

    let signum = signum as u32;

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
