use alloc::sync::Arc;
use bitflags::bitflags;

use crate::{copy_to_user, kernel::{ipc::Pipe, scheduler::current, task::fdtable::FDFlags}};

use super::SyscallRet;

bitflags! {
    struct PipeFlags: usize {
        const O_CLOEXEC = 0x80000;
    }
}

#[repr(C)]
struct PipeFD {
    read_fd: i32,
    write_fd: i32,
}

pub fn pipe(uptr_pipefd: usize, flags: usize) -> SyscallRet {
    let flags = PipeFlags::from_bits_truncate(flags);
    let fd_flags = FDFlags{
        cloexec: flags.contains(PipeFlags::O_CLOEXEC),
    };
    
    let (read_end, write_end) = Pipe::create(4096); // Create a pipe with a buffer size of 4096 bytes
    let read_end = Arc::new(read_end);
    let write_end = Arc::new(write_end);

    let (read_fd, write_fd);
    {
        let mut fdtable = current::fdtable().lock();
        read_fd = fdtable.push(read_end, fd_flags)?;
        write_fd = fdtable.push(write_end, fd_flags)?;
    }

    copy_to_user!(
        uptr_pipefd, 
        PipeFD { 
            read_fd: read_fd as i32, 
            write_fd: write_fd as i32
        }
    )?;

    Ok(0)
}
