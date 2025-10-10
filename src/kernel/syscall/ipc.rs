use alloc::sync::Arc;
use bitflags::bitflags;

use crate::kernel::config;
use crate::kernel::ipc::Pipe;
use crate::kernel::scheduler::current;
use crate::kernel::task::fdtable::FDFlags;
use crate::kernel::api;
use crate::{copy_from_user, copy_to_user};

use super::SyscallRet;

bitflags! {
    struct PipeFlags: usize {
        const O_CLOEXEC = 0x80000;
    }
}

pub fn pipe(uptr_pipefd: usize, flags: usize) -> SyscallRet {
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

    copy_to_user!(
        uptr_pipefd, [read_fd as i32, write_fd as i32]
    )?;

    Ok(0)
}

pub fn rt_sigprocmask(_how: usize, _set: usize, _oldset: usize) -> SyscallRet {
    Ok(0)
}

pub fn rt_sigaction(signum: usize, uptr_act: usize, uptr_oldact: usize, sigsetsize: usize) -> SyscallRet {
    assert!(sigsetsize == core::mem::size_of::<api::sigset_t>());

    let signum = signum as u32;

    let mut signal_handler = current::signal_handler().lock();
    if uptr_oldact != 0 {
        let old_action = signal_handler.get_sigaction(signum)?;
        let old_action: api::Sigaction = old_action.into();
        copy_to_user!(uptr_oldact, old_action)?;
    }

    if uptr_act != 0 {
        let new_action = api::Sigaction::empty();
        copy_from_user!(uptr_act, new_action)?;
        signal_handler.set_sigaction(signum, &new_action.into())?;
    }

    Ok(0)
}

pub fn rt_sig_return() -> SyscallRet {
    // current::tcb().signal_return();
    Ok(0)
}
