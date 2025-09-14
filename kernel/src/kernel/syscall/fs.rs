use alloc::sync::Arc;
use bitflags::bitflags;

use crate::kernel::errno::Errno;
use crate::kernel::scheduler::*;
use crate::fs::vfs;
use crate::fs::file::FileFlags;
use crate::fs::FileStat;
use crate::{ktrace, kdebug};

use super::def::*;

const BUFFER_SIZE: usize = 1024;

bitflags! {
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub struct OpenFlags: usize {
        const O_RDONLY    = 0x0000;
        const O_WRONLY    = 0x0001;
        const O_RDWR      = 0x0002;
        const O_CREAT     = 0x0040;
        const O_EXCL      = 0x0080;
        const O_NOCTTY    = 0x0100;
        const O_TRUNC     = 0x0200;
        const O_APPEND    = 0x0400;
        const O_NONBLOCK  = 0x0800;
        const O_DSYNC     = 0x1000;
        const FASYNC      = 0x2000;
        const O_DIRECT    = 0x4000;
        const O_LARGEFILE = 0x8000;
        const O_DIRECTORY = 0x10000;
        const O_NOFOLLOW  = 0x20000;
        const O_CLOEXEC   = 0x80000;
    }
}

pub fn dup(oldfd: usize) -> Result<usize, Errno> {
    let file = current::fdtable().get(oldfd)?;
    let newfd = current::fdtable().push(file.clone())?;
    Ok(newfd)
}

pub fn fcntl64(_fd: usize, _cmd: usize, _arg: usize) -> Result<usize, Errno> {
    Ok(0)
}

pub fn openat(dirfd: usize, user_filename: usize, _flags: usize, _mode: usize) -> Result<usize, Errno> {
    if dirfd as isize != AT_FDCWD {
        return Err(Errno::ENOSYS); // Currently only AT_FDCWD is supported
    }

    let open_flags = OpenFlags::from_bits(_flags).ok_or(Errno::EINVAL)?;
    let flags = FileFlags {
        writable: open_flags.contains(OpenFlags::O_WRONLY) || open_flags.contains(OpenFlags::O_RDWR),
        cloexec: open_flags.contains(OpenFlags::O_CLOEXEC),
    };

    let path = current::get_user_string(user_filename)?;

    kdebug!("openat called: path={}, flags={:?}", path, open_flags);

    let file = current::with_pwd(|pwd| vfs::openat_file(pwd, &path, flags))?;
    let fd = current::fdtable().push(Arc::new(file))?;

    Ok(fd)
}

pub fn read(fd: usize, user_buffer: usize, count: usize) -> Result<usize, Errno> {
    let file = current::fdtable().get(fd)?;

    let addrspace = current::addrspace();
        
    let mut buffer = [0u8; BUFFER_SIZE];
    let mut total_read = 0;
    let mut left = count;

    while left > 0 {
        let to_read = core::cmp::min(left, BUFFER_SIZE);
        let bytes_read = file.read(&mut buffer[..to_read])?;
        if bytes_read == 0 {
            break; // EOF
        }
        addrspace.copy_to_user(user_buffer + total_read, &buffer[..bytes_read])
            .map_err(|_| Errno::EFAULT)?;
        total_read += bytes_read;
        left -= bytes_read;
    }

    Ok(total_read)
}

pub fn write(fd: usize, uptr_buffer: usize, mut count: usize) -> Result<usize, Errno> {
    let file = current::fdtable().get(fd)?;

    let addrspace = current::addrspace();
    let mut written = 0;

    // ktrace!("Writing");

    while count != 0 {
        let mut buffer = [0u8; BUFFER_SIZE];
        let to_write = core::cmp::min(count, BUFFER_SIZE);
        addrspace.copy_from_user(uptr_buffer + written, &mut buffer[..to_write])
            .map_err(|_| Errno::EFAULT)?;
        
        file.write(&buffer[..to_write]).map_err(|_| Errno::EIO)?;

        count -= to_write;
        written += to_write;
    }
    
    Ok(written)
}

#[repr(C)]
pub struct IOVec {
    pub base: usize,
    pub len: usize,
}

pub fn writev(fd: usize, iov: usize, iovcnt: usize) -> Result<usize, Errno> {
    let file = current::fdtable().get(fd)?;

    let addrspace = current::addrspace();

    let mut total_written = 0;

    for i in 0..iovcnt {
        let mut iov_buf = [0u8; core::mem::size_of::<IOVec>()];
        addrspace.copy_from_user(iov + i * core::mem::size_of::<IOVec>(), &mut iov_buf)
            .map_err(|_| Errno::EFAULT)?;
        let iov = unsafe { &*(iov_buf.as_ptr() as *const IOVec) };

        let mut written = 0usize;
        let mut remaining = iov.len;
        let mut buffer = [0u8; BUFFER_SIZE];
        while remaining != 0 {
            let to_write = core::cmp::min(remaining, BUFFER_SIZE);
            addrspace.copy_from_user(iov.base + written, &mut buffer[..to_write])
                .map_err(|_| Errno::EFAULT)?;

            let bytes_written = file.write(&buffer[..to_write]).map_err(|_| Errno::EIO)?;
            if bytes_written != to_write {
                break; // EOF
            }

            remaining -= to_write;
            written += to_write;
        }

        total_written += written;
    }

    Ok(total_written)
}

pub fn close(fd: usize) -> Result<usize, Errno> {
    current::fdtable().close(fd)?;

    Ok(0)
}

pub fn ioctl(fd: usize, request: usize, arg: usize) -> Result<usize, Errno> {
    let file = current::fdtable().get(fd)?;

    file.ioctl(request, arg)
}

// TODO: Implement faccessat
pub fn faccessat(_dirfd: usize, _uptr_path: usize, _mode: usize) -> Result<usize, Errno> {
    Ok(0)
}

pub fn fstatat(dirfd: usize, uptr_path: usize, uptr_statbuf: usize, _flags: usize) -> Result<usize, Errno> {
    let path = current::get_user_string(uptr_path)?;

    let kstat = if dirfd as isize == AT_FDCWD {
        current::with_pwd(|pwd| vfs::openat_file(pwd, &path, FileFlags::dontcare()))?
    } else {
        vfs::open_file(&path, FileFlags::dontcare())?
    }.fstat()?;

    let buffer = unsafe {
        core::slice::from_raw_parts(
            &kstat as *const _ as *const u8,
            core::mem::size_of::<FileStat>()
        )
    };

    current::copy_to_user(uptr_statbuf, buffer)?;

    ktrace!("fstatat: path={}, st_size={}, st_mode={:#o}", path, kstat.st_size, kstat.st_mode);

    Ok(0)
}

pub fn newfstat(fd: usize, uptr_statbuf: usize) -> Result<usize, Errno> {
    let file = current::fdtable().get(fd)?;

    let kstat = file.fstat()?;
    let buffer = unsafe {
        core::slice::from_raw_parts(
            &kstat as *const _ as *const u8,
            core::mem::size_of::<FileStat>()
        )
    };

    current::copy_to_user(uptr_statbuf, buffer)?;

    ktrace!("newfstat: fd={}, st_size={}, st_mode={:#o}", fd, kstat.st_size, kstat.st_mode);

    Ok(0)
}
