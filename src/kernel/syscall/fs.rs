use alloc::sync::Arc;
use bitflags::bitflags;
use core::time::Duration;
use core::usize;
use fixedstr::str256;
use num_enum::TryFromPrimitive;

use crate::driver;
use crate::fs::file::{File, FileFlags, FileOps, SeekWhence};
use crate::fs::{Dentry, FileType, Mode, Owner, Perm, PermFlags, vfs};
use crate::kernel::errno::{Errno, SysResult};
use crate::kernel::ipc::{KSiFields, Pipe, SiCode, signum};
use crate::kernel::scheduler::current::{copy_from_user, copy_to_user};
use crate::kernel::scheduler::*;
use crate::kernel::syscall::uptr::{UArray, UBuffer, UPtr, UString, UserPointer};
use crate::kernel::syscall::{SyscallRet, UserStruct, utils};
use crate::kernel::task::fdtable::FDFlags;
use crate::kernel::uapi::{Dirent, DirentType, FileStat, OpenFlags, Statfs, Timespec, Uid};

use super::def::*;

pub fn dup(oldfd: usize) -> SyscallRet {
    let mut fdtable = current::fdtable().lock();
    fdtable.dup(oldfd, None, FDFlags::empty())
}

pub fn dup3(oldfd: usize, newfd: usize, flags: usize) -> SyscallRet {
    let flags = OpenFlags::from_bits(flags).ok_or(Errno::EINVAL)?;
    let fd_flags = FDFlags {
        cloexec: flags.contains(OpenFlags::O_CLOEXEC),
    };
    let mut fdtable = current::fdtable().lock();
    fdtable.dup3(oldfd, newfd, fd_flags)
}

#[allow(non_camel_case_types)]
#[derive(TryFromPrimitive)]
#[repr(usize)]
pub enum FcntlCmd {
    F_DUPFD = 0,
    F_GETFD = 1,
    F_SETFD = 2,
    F_GETFL = 3,
    F_SETFL = 4,
    F_DUPFD_CLOEXEC = 1030,
    F_SETPIPE_SZ = 1031,
    F_GETPIPE_SZ = 1032,
}

bitflags! {
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub struct FDArgs: usize {
        const FD_CLOEXEC = 1;
    }
}

pub fn fcntl64(fd: usize, cmd: usize, arg: usize) -> SyscallRet {
    match FcntlCmd::try_from(cmd).map_err(|_| Errno::EINVAL)? {
        FcntlCmd::F_GETFL => {
            let file = current::fdtable().lock().get(fd)?;
            let flags = file.flags();
            let mut open_flags = OpenFlags::O_RDONLY;
            if flags.readable && flags.writable {
                open_flags = OpenFlags::O_RDWR;
            } else if flags.writable {
                open_flags = OpenFlags::O_WRONLY;
            }
            if !flags.blocked {
                open_flags |= OpenFlags::O_NONBLOCK;
            }
            Ok(open_flags.bits())
        }

        FcntlCmd::F_SETFL => {
            let file = current::fdtable().lock().get(fd)?;
            let flags = OpenFlags::from_bits(arg).ok_or(Errno::EINVAL)?;
            let old_flags = file.flags();
            file.set_flags(FileFlags {
                blocked: !flags.contains(OpenFlags::O_NONBLOCK),
                append: flags.contains(OpenFlags::O_APPEND),
                ..old_flags
            });
            current::fdtable().lock().set(
                fd,
                file,
                FDFlags {
                    cloexec: flags.contains(OpenFlags::O_CLOEXEC),
                },
            )?;

            Ok(0)
        }

        FcntlCmd::F_GETFD => {
            let fdtable = current::fdtable().lock();
            let fdflags = fdtable.get_fd_flags(fd)?;
            let mut flags = FDArgs::empty();
            if fdflags.cloexec {
                flags |= FDArgs::FD_CLOEXEC;
            }
            Ok(flags.bits())
        }

        FcntlCmd::F_SETFD => {
            let flags = FDArgs::from_bits(arg).ok_or(Errno::EINVAL)?;

            let mut fdtable = current::fdtable().lock();
            let mut fdflags = fdtable.get_fd_flags(fd)?;
            fdflags.cloexec = flags.contains(FDArgs::FD_CLOEXEC);
            fdtable.set_fd_flags(fd, fdflags)?;

            Ok(0)
        }

        FcntlCmd::F_DUPFD_CLOEXEC => {
            let mut fdtable = current::fdtable().lock();
            fdtable.dup(fd, Some(arg), FDFlags { cloexec: true })
        }

        FcntlCmd::F_SETPIPE_SZ => {
            let file = current::fdtable().lock().get(fd)?;
            let pipe = file.downcast_ref::<Pipe>().ok_or(Errno::EINVAL)?;
            pipe.set_pipe_size(arg)
        }

        FcntlCmd::F_GETPIPE_SZ => {
            let file = current::fdtable().lock().get(fd)?;
            let pipe = file.downcast_ref::<Pipe>().ok_or(Errno::EINVAL)?;
            Ok(pipe.get_pipe_size())
        }

        _ => Err(Errno::EINVAL),
    }
}

pub fn openat(dirfd: usize, uptr_filename: UString, flags: usize, mode: usize) -> SyscallRet {
    uptr_filename.should_not_null()?;

    let open_flags = OpenFlags::from_bits(flags).ok_or(Errno::EINVAL)?;
    if open_flags.contains(OpenFlags::O_DIRECTORY) && open_flags.contains(OpenFlags::O_CREATE) {
        return Err(Errno::EINVAL);
    }
    let acc_mode = flags & (OpenFlags::O_WRONLY.bits() | OpenFlags::O_RDWR.bits());
    let (readable, writable) = match acc_mode {
        0 => (true, false), // O_RDONLY
        1 => (false, true), // O_WRONLY
        2 => (true, true),  // O_RDWR
        _ => return Err(Errno::EINVAL),
    };
    let file_flags = FileFlags {
        writable,
        readable,
        blocked: !open_flags.contains(OpenFlags::O_NONBLOCK),
        append: open_flags.contains(OpenFlags::O_APPEND),
        direct: open_flags.contains(OpenFlags::O_DIRECT),
    };
    let fd_flags = FDFlags {
        cloexec: open_flags.contains(OpenFlags::O_CLOEXEC),
    };

    let path = uptr_filename.read_fixed()?;

    let helper = |parent: &Arc<Dentry>| {
        if open_flags.contains(OpenFlags::O_TMPFILE) {
            if !writable {
                return Err(Errno::EINVAL);
            }

            let dentry = vfs::load_dentry_at(parent, &path)?;
            return vfs::create_temp(
                &dentry,
                file_flags,
                Mode::from_bits(mode as u32 & 0o7777 & !current::umask()).ok_or(Errno::EINVAL)? | Mode::S_IFREG,
            );
        }

        let mut perm_flags = PermFlags::empty();
        if readable {
            perm_flags.insert(PermFlags::R);
        }
        if writable {
            perm_flags.insert(PermFlags::W);
        }

        match vfs::openat_file(parent, &path, file_flags, &Perm::current(perm_flags)) {
            Ok(file) => {
                if open_flags.contains(OpenFlags::O_CREATE) && open_flags.contains(OpenFlags::O_EXCL) {
                    return Err(Errno::EEXIST);
                }
                Ok(file)
            }
            Err(e) => {
                if e == Errno::ENOENT && open_flags.contains(OpenFlags::O_CREATE) {
                    // Create the file
                    let mode =
                        Mode::from_bits(mode as u32 & 0o7777 & !current::umask()).ok_or(Errno::EINVAL)? | Mode::S_IFREG;
                    let (parent_dentry, child_name) = vfs::load_parent_dentry_at(parent, &path)?.unwrap(); // SAFETY: The root must exist
                    vfs::create_file(
                        &parent_dentry,
                        &child_name,
                        file_flags,
                        mode,
                        Owner::new(current::fsuid(), current::fsgid()),
                    )
                } else {
                    Err(e)
                }
            }
        }
    };

    let file = if dirfd as isize == AT_FDCWD {
        current::with_cwd(|cwd| helper(&cwd))?
    } else {
        helper(vfs::get_root_dentry())?
    };

    if open_flags.contains(OpenFlags::O_DIRECTORY) {
        let inode = file.get_inode().ok_or(Errno::ENOTDIR)?;
        if inode.inode_type()? != FileType::Directory {
            return Err(Errno::ENOTDIR);
        }
    }

    if writable && open_flags.contains(OpenFlags::O_TRUNC) {
        if let Some(inode) = file.get_inode() {
            if inode.inode_type()? == FileType::Regular {
                inode.truncate(0)?;
            }
        }
    }

    let fd = current::fdtable().lock().push(file, fd_flags)?;

    Ok(fd)
}

pub fn read(fd: usize, ubuf: UBuffer, count: usize) -> SyscallRet {
    let file = current::fdtable().lock().get(fd)?;

    if !file.readable() {
        return Err(Errno::EBADF);
    }

    if count == 0 {
        return Ok(0);
    }

    ubuf.should_not_null()?;

    let ubuf = ubuf.to_uaddrspace_buffer(count);

    let total_read = file.read_to_user(&ubuf)?;

    Ok(total_read)
}

pub fn readlinkat(dirfd: usize, uptr_path: UString, ubuf: UBuffer, bufsize: usize) -> SyscallRet {
    uptr_path.should_not_null()?;
    ubuf.should_not_null()?;

    let path = uptr_path.read_fixed()?;

    if let Some((parent, child)) = if dirfd as isize == AT_FDCWD {
        current::with_cwd(|cwd| vfs::load_parent_dentry_at(&cwd, &path))?
    } else {
        vfs::load_parent_dentry_at(
            current::fdtable()
                .lock()
                .get(dirfd)?
                .get_dentry()
                .ok_or(Errno::ENOTDIR)?,
            &path,
        )?
    } {
        let mut buffer = [0u8; 255];
        if let Some(size) = parent.readlink(child, &mut buffer)? {
            let path = core::str::from_utf8(&buffer[..size]).map_err(|_| Errno::EINVAL)?;
            let to_write = core::cmp::min(path.len(), bufsize);
            ubuf.write(0, &path.as_bytes()[..to_write])?;
            return Ok(to_write);
        } else {
            return Err(Errno::EINVAL); // Not a symlink
        }
    } else {
        return Err(Errno::EINVAL); // Root is a directory, cannot be a symlink
    }
}

pub fn write(fd: usize, ubuf: UBuffer, count: usize) -> SyscallRet {
    if count == 0 {
        return Ok(0);
    }

    ubuf.should_not_null()?;

    let file = current::fdtable().lock().get(fd)?;
    if !file.writable() {
        return Err(Errno::EBADF);
    }

    let ubuf = ubuf.to_uaddrspace_buffer(count);
    let written = file.write_from_user(&ubuf)?;

    Ok(written)
}

#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct IOVec {
    pub base: usize,
    pub len: usize,
}

impl UserStruct for IOVec {}

fn check_positional_io(file: &Arc<dyn FileOps>) -> SysResult<()> {
    file.seek(0, SeekWhence::CUR).map(|_| ())
}

pub fn readv(fd: usize, uptr_iov: UPtr<IOVec>, iovcnt: usize) -> SyscallRet {
    if (iovcnt as isize) < 0 {
        return Err(Errno::EINVAL);
    }

    let file = current::fdtable().lock().get(fd)?;
    if !file.readable() {
        return Err(Errno::EBADF);
    }

    if iovcnt == 0 {
        return Ok(0);
    }

    if (iovcnt as isize) < 0 {
        return Err(Errno::EINVAL);
    }

    uptr_iov.should_not_null()?;

    let mut total_read = 0;

    for i in 0..iovcnt {
        let iov = match uptr_iov.add(i).read() {
            Ok(iov) => iov,
            Err(e) => {
                if total_read > 0 {
                    break;
                } else {
                    return Err(e);
                }
            }
        };
        if (iov.len as isize) < 0 {
            if total_read > 0 {
                break;
            }
            return Err(Errno::EINVAL);
        }

        let mut read = 0usize;
        let mut remaining = iov.len;
        let mut buffer = [0u8; BUFFER_SIZE];
        while remaining != 0 {
            let to_read = core::cmp::min(remaining, BUFFER_SIZE);
            let bytes_read = match file.read(&mut buffer[..to_read]) {
                Ok(n) => n,
                Err(e) => {
                    if total_read + read > 0 {
                        return Ok(total_read + read);
                    }
                    return Err(e);
                }
            };
            if bytes_read == 0 {
                break; // EOF
            }

            if copy_to_user::buffer(iov.base + read, &buffer[..bytes_read]).is_err() {
                if total_read + read > 0 {
                    return Ok(total_read + read);
                }
                return Err(Errno::EFAULT);
            }

            remaining -= bytes_read;
            read += bytes_read;
        }

        total_read += read;
    }

    Ok(total_read)
}

pub fn preadv(fd: usize, uptr_iov: UPtr<IOVec>, iovcnt: usize, pos: usize) -> SyscallRet {
    if (iovcnt as isize) < 0 || (pos as isize) < 0 {
        return Err(Errno::EINVAL);
    }

    let file = current::fdtable().lock().get(fd)?;
    check_positional_io(&file)?;
    if !file.readable() {
        return Err(Errno::EBADF);
    }

    if iovcnt == 0 {
        return Ok(0);
    }

    uptr_iov.should_not_null()?;

    let mut total_read = 0usize;
    let mut offset = 0usize;

    for i in 0..iovcnt {
        let iov = match uptr_iov.add(i).read() {
            Ok(iov) => iov,
            Err(e) => {
                if total_read > 0 {
                    break;
                } else {
                    return Err(e);
                }
            }
        };
        if (iov.len as isize) < 0 {
            if total_read > 0 {
                break;
            }
            return Err(Errno::EINVAL);
        }

        let mut read = 0usize;
        let mut remaining = iov.len;
        let mut buffer = [0u8; BUFFER_SIZE];
        while remaining != 0 {
            let to_read = core::cmp::min(remaining, BUFFER_SIZE);
            let read_pos = pos.checked_add(offset).ok_or(Errno::EINVAL)?;
            let bytes_read = match file.pread(&mut buffer[..to_read], read_pos) {
                Ok(n) => n,
                Err(e) => {
                    if total_read + read > 0 {
                        return Ok(total_read + read);
                    }
                    return Err(e);
                }
            };
            if bytes_read == 0 {
                break; // EOF
            }

            if copy_to_user::buffer(iov.base + read, &buffer[..bytes_read]).is_err() {
                if total_read + read > 0 {
                    return Ok(total_read + read);
                }
                return Err(Errno::EFAULT);
            }

            remaining -= bytes_read;
            read += bytes_read;
            offset = offset.checked_add(bytes_read).ok_or(Errno::EINVAL)?;
        }

        total_read += read;
    }

    Ok(total_read)
}

pub fn preadv2(
    fd: usize,
    uptr_iov: UPtr<IOVec>,
    iovcnt: usize,
    pos_l: usize,
    pos_h: usize,
    flags: usize,
) -> SyscallRet {
    if (iovcnt as isize) < 0 {
        return Err(Errno::EINVAL);
    }

    if flags != 0 {
        return Err(Errno::EOPNOTSUPP);
    }

    let pos_u64 = (((pos_h & 0xffff_ffff) as u64) << 32) | ((pos_l & 0xffff_ffff) as u64);
    let pos = pos_u64 as usize;

    // Linux preadv2 semantics: offset == -1 means use and advance the current file offset.
    if pos == usize::MAX {
        return readv(fd, uptr_iov, iovcnt);
    }

    preadv(fd, uptr_iov, iovcnt, pos)
}

pub fn pread64(fd: usize, ubuf: UBuffer, count: usize, pos: usize) -> SyscallRet {
    let file = current::fdtable().lock().get(fd)?;

    if count == 0 {
        return Ok(0);
    }

    if (count as isize) < 0 || (pos as isize) < 0 {
        return Err(Errno::EINVAL);
    }

    check_positional_io(&file)?;
    if !file.readable() {
        return Err(Errno::EBADF);
    }

    let mut written = 0;
    let mut buffer = [0u8; BUFFER_SIZE];
    let mut left = count;

    while left != 0 {
        let to_read = core::cmp::min(left, BUFFER_SIZE);
        let bytes_read = file.pread(&mut buffer[..to_read], pos + (count - left))?;
        if bytes_read == 0 {
            break; // EOF
        }

        ubuf.write(count - left, &buffer[..bytes_read])?;

        left -= bytes_read;
        written += bytes_read;

        if bytes_read < to_read {
            break; // EOF
        }
    }

    Ok(written)
}

pub fn pwrite64(fd: usize, ubuf: UBuffer, count: usize, pos: usize) -> SyscallRet {
    if count == 0 {
        return Ok(0);
    }

    if (pos as isize) < 0 || (count as isize) < 0 {
        return Err(Errno::EINVAL);
    }
    if ubuf.is_null() {
        return Err(Errno::EFAULT);
    }

    let file = current::fdtable().lock().get(fd)?;
    check_positional_io(&file)?;
    if !file.writable() {
        return Err(Errno::EBADF);
    }

    let mut written = 0;
    let mut buffer = [0u8; BUFFER_SIZE];
    let mut left = count;

    while left != 0 {
        let to_write = core::cmp::min(left, BUFFER_SIZE);
        ubuf.read(count - left, &mut buffer[..to_write])?;

        let bytes_written = file.pwrite(&buffer[..to_write], pos + (count - left))?;
        if bytes_written == 0 {
            break; // EOF
        }

        left -= bytes_written;
        written += bytes_written;

        if bytes_written < to_write {
            break; // EOF
        }
    }

    Ok(written)
}

pub fn pwritev(fd: usize, uptr_iov: UPtr<IOVec>, iovcnt: usize, pos: usize) -> SyscallRet {
    if (iovcnt as isize) < 0 || (pos as isize) < 0 {
        return Err(Errno::EINVAL);
    }

    let file = current::fdtable().lock().get(fd)?;
    check_positional_io(&file)?;
    if !file.writable() {
        return Err(Errno::EBADF);
    }

    if iovcnt == 0 {
        return Ok(0);
    }

    uptr_iov.should_not_null()?;

    let mut total_written = 0usize;
    let mut offset = 0usize;

    for i in 0..iovcnt {
        let iov = match uptr_iov.add(i).read() {
            Ok(iov) => iov,
            Err(e) => {
                if total_written > 0 {
                    break;
                } else {
                    return Err(e);
                }
            }
        };

        if iov.len == 0 {
            continue;
        }
        if (iov.len as isize) < 0 {
            if total_written > 0 {
                break;
            }
            return Err(Errno::EINVAL);
        }

        let mut written = 0usize;
        let mut remaining = iov.len;
        let mut buffer = [0u8; BUFFER_SIZE];
        while remaining != 0 {
            let to_write = core::cmp::min(remaining, BUFFER_SIZE);
            if copy_from_user::buffer(iov.base + written, &mut buffer[..to_write]).is_err() {
                if total_written + written > 0 {
                    return Ok(total_written + written);
                }
                return Err(Errno::EFAULT);
            }

            let write_pos = pos.checked_add(offset).ok_or(Errno::EINVAL)?;
            match file.pwrite(&buffer[..to_write], write_pos) {
                Ok(bytes_written) => {
                    remaining -= bytes_written;
                    written += bytes_written;
                    offset = offset.checked_add(bytes_written).ok_or(Errno::EINVAL)?;
                    if bytes_written != to_write {
                        return Ok(total_written + written);
                    }
                }
                Err(e) => {
                    if total_written + written > 0 {
                        return Ok(total_written + written);
                    }
                    return Err(e);
                }
            }
        }

        total_written += written;
    }

    Ok(total_written)
}

pub fn writev(fd: usize, uptr_iov: UPtr<IOVec>, iovcnt: usize) -> SyscallRet {
    if (iovcnt as isize) < 0 {
        return Err(Errno::EINVAL);
    }

    let file = current::fdtable().lock().get(fd)?;
    if !file.writable() {
        return Err(Errno::EBADF);
    }

    if iovcnt == 0 {
        return Ok(0);
    }

    uptr_iov.should_not_null()?;

    let mut total_written = 0;

    for i in 0..iovcnt {
        let iov = match uptr_iov.add(i).read() {
            Ok(iov) => iov,
            Err(e) => {
                if total_written > 0 {
                    break;
                } else {
                    return Err(e);
                }
            }
        };

        if iov.len == 0 {
            continue;
        }
        if (iov.len as isize) < 0 {
            if total_written > 0 {
                break;
            }
            return Err(Errno::EINVAL);
        }

        let mut written = 0usize;
        let mut remaining = iov.len;
        let mut buffer = [0u8; BUFFER_SIZE];
        while remaining != 0 {
            let to_write = core::cmp::min(remaining, BUFFER_SIZE);
            if copy_from_user::buffer(iov.base + written, &mut buffer[..to_write]).is_err() {
                if total_written + written > 0 {
                    return Ok(total_written + written);
                }
                return Err(Errno::EFAULT);
            }

            match file.write(&buffer[..to_write]) {
                Ok(bytes_written) => {
                    remaining -= bytes_written;
                    written += bytes_written;
                    if bytes_written != to_write {
                        return Ok(total_written + written);
                    }
                }
                Err(e) => {
                    if total_written + written > 0 {
                        return Ok(total_written + written);
                    }
                    return Err(e);
                }
            }
        }

        total_written += written;
    }

    Ok(total_written)
}

pub fn pwritev2(
    fd: usize,
    uptr_iov: UPtr<IOVec>,
    iovcnt: usize,
    pos_l: usize,
    pos_h: usize,
    flags: usize,
) -> SyscallRet {
    if (iovcnt as isize) < 0 {
        return Err(Errno::EINVAL);
    }

    if flags != 0 {
        return Err(Errno::EOPNOTSUPP);
    }

    let pos_u64 = (((pos_h & 0xffff_ffff) as u64) << 32) | ((pos_l & 0xffff_ffff) as u64);
    let pos = pos_u64 as usize;

    // Linux pwritev2 semantics: offset == -1 means use and advance the current file offset.
    if pos == usize::MAX {
        return writev(fd, uptr_iov, iovcnt);
    }

    pwritev(fd, uptr_iov, iovcnt, pos)
}

pub fn lseek(fd: usize, offset: usize, how: usize) -> SyscallRet {
    let file = current::fdtable().lock().get(fd)?;

    let how = match how {
        0 => SeekWhence::BEG,
        1 => SeekWhence::CUR,
        2 => SeekWhence::END,
        _ => return Err(Errno::EINVAL),
    };

    file.seek(offset as isize, how)
}

pub fn close(fd: usize) -> Result<usize, Errno> {
    // Drop Arc<dyn FileOps> without lock
    let file = current::fdtable().lock().take(fd)?;
    drop(file);

    Ok(0)
}

bitflags! {
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub struct CloseRangeFlags: usize {
        const UNSHARE = 1 << 1;
        const CLOEXEC = 1 << 2;
    }
}

pub fn close_range(fd: usize, max_fd: usize, flags: usize) -> SyscallRet {
    if fd > max_fd {
        return Err(Errno::EINVAL);
    }

    let flags = CloseRangeFlags::from_bits(flags).ok_or(Errno::EINVAL)?;

    if flags.contains(CloseRangeFlags::UNSHARE) {
        current::tcb().unshare_fdtable();
    }

    let mut fdtable = current::fdtable().lock();
    if flags.contains(CloseRangeFlags::CLOEXEC) {
        for i in fd..=max_fd {
            if let Ok(fdflags) = fdtable.get_fd_flags(i) {
                fdtable.set_fd_flags(
                    i,
                    FDFlags {
                        cloexec: true,
                        ..fdflags
                    },
                )?;
            }
        }
    } else {
        for i in fd..=max_fd {
            let _ = fdtable.take(i);
        }
    }

    Ok(0)
}

pub fn sendfile(out_fd: usize, in_fd: usize, uptr_offset: UPtr<usize>, count: usize) -> SyscallRet {
    let mut fdtable = current::fdtable().lock();
    let out_file = fdtable.get(out_fd)?;
    let in_file = fdtable.get(in_fd)?.downcast_arc::<File>().map_err(|_| Errno::EINVAL)?;
    drop(fdtable); // Release lock early

    if !out_file.writable() {
        return Err(Errno::EBADF);
    }
    if !in_file.readable() {
        return Err(Errno::EBADF);
    }

    let in_file_offset = in_file.seek(0, SeekWhence::CUR)?;
    let mut local_offset = if uptr_offset.is_null() {
        in_file_offset
    } else {
        utils::should_not_be_negative(uptr_offset.read()?)?
    };

    let mut total_sent = 0;
    let mut left = count;

    let mut buffer = [0u8; BUFFER_SIZE];

    while left > 0 {
        let to_read = core::cmp::min(left, BUFFER_SIZE);
        let bytes_read = in_file.read_at(&mut buffer[..to_read], local_offset)?;
        if bytes_read == 0 {
            break; // EOF
        }

        let bytes_written = out_file.write(&buffer[..bytes_read])?;
        if bytes_written == 0 {
            break; // Can't write more
        }

        local_offset += bytes_read;
        total_sent += bytes_written;
        left -= bytes_written;

        if bytes_read < to_read {
            break; // EOF
        }

        if bytes_written < bytes_read {
            break; // Can't write more
        }
    }

    if uptr_offset.is_null() {
        in_file.seek(local_offset as isize, SeekWhence::BEG)?;
    } else {
        uptr_offset.write(local_offset)?;
    }

    Ok(total_sent)
}

bitflags! {
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub struct SpliceFlags: usize {
        const MOVE = 1;
        const NONBLOCK = 1 << 1;
        const MORE = 1 << 2;
        const GIFT = 1 << 3;
    }
}

fn splice_offsets(file: &Arc<dyn FileOps>, uptr_offset: UPtr<usize>, is_pipe: bool) -> SysResult<Option<usize>> {
    if uptr_offset.is_null() {
        return Ok(None);
    }

    if is_pipe {
        return Err(Errno::ESPIPE);
    }

    let offset = uptr_offset.read()?;
    if (offset as isize) < 0 {
        return Err(Errno::EINVAL);
    }

    check_positional_io(file)?;
    Ok(Some(offset))
}

fn splice_advance_offset(offset: &mut Option<usize>, delta: usize) -> SysResult<()> {
    if let Some(pos) = offset {
        *pos = pos.checked_add(delta).ok_or(Errno::EINVAL)?;
    }
    Ok(())
}

fn splice_pipe_blocked(file: &Arc<dyn FileOps>, flags: SpliceFlags) -> Option<bool> {
    file.downcast_ref::<Pipe>()
        .map(|_| !flags.contains(SpliceFlags::NONBLOCK))
}

fn splice_read_chunk(
    file: &Arc<dyn FileOps>,
    offset: Option<usize>,
    flags: SpliceFlags,
    buf: &mut [u8],
) -> SysResult<usize> {
    match offset {
        Some(pos) => file.pread(buf, pos),
        None => match (file.downcast_ref::<Pipe>(), splice_pipe_blocked(file, flags)) {
            (Some(pipe), Some(blocked)) => pipe.read_with_blocked(buf, blocked),
            _ => file.read(buf),
        },
    }
}

fn splice_write_chunk(
    file: &Arc<dyn FileOps>,
    offset: Option<usize>,
    flags: SpliceFlags,
    buf: &[u8],
) -> SysResult<usize> {
    match offset {
        Some(pos) => file.pwrite(buf, pos),
        None => match (file.downcast_ref::<Pipe>(), splice_pipe_blocked(file, flags)) {
            (Some(pipe), Some(blocked)) => pipe.write_with_blocked(buf, blocked),
            _ => file.write(buf),
        },
    }
}

pub fn splice(
    in_fd: usize,
    uptr_off_in: UPtr<usize>,
    out_fd: usize,
    uptr_off_out: UPtr<usize>,
    len: usize,
    flags: usize,
) -> SyscallRet {
    let flags = SpliceFlags::from_bits(flags).ok_or(Errno::EINVAL)?;

    let mut fdtable = current::fdtable().lock();
    let in_file = fdtable.get(in_fd)?;
    let out_file = fdtable.get(out_fd)?;
    drop(fdtable);

    if !in_file.readable() || !out_file.writable() {
        return Err(Errno::EBADF);
    }
    if out_file.flags().append {
        return Err(Errno::EINVAL);
    }

    let in_is_pipe = in_file.downcast_ref::<Pipe>().is_some();
    let out_is_pipe = out_file.downcast_ref::<Pipe>().is_some();
    if !in_is_pipe && !out_is_pipe {
        return Err(Errno::EINVAL);
    }

    if len == 0 {
        return Ok(0);
    }

    let mut in_offset = splice_offsets(&in_file, uptr_off_in, in_is_pipe)?;
    let mut out_offset = splice_offsets(&out_file, uptr_off_out, out_is_pipe)?;

    let mut total_moved = 0usize;
    let mut left = len;
    let mut buffer = [0u8; BUFFER_SIZE];

    while left > 0 {
        let to_read = core::cmp::min(left, BUFFER_SIZE);
        let bytes_read = match splice_read_chunk(&in_file, in_offset, flags, &mut buffer[..to_read]) {
            Ok(n) => n,
            Err(e) => {
                if total_moved > 0 {
                    break;
                }
                return Err(e);
            }
        };
        if bytes_read == 0 {
            break;
        }

        let mut moved_from_chunk = 0usize;
        while moved_from_chunk < bytes_read {
            let write_buf = &buffer[moved_from_chunk..bytes_read];
            let bytes_written = match splice_write_chunk(&out_file, out_offset, flags, write_buf) {
                Ok(n) => n,
                Err(e) => {
                    if total_moved + moved_from_chunk > 0 {
                        break;
                    }
                    return Err(e);
                }
            };

            if bytes_written == 0 {
                break;
            }

            moved_from_chunk += bytes_written;
            splice_advance_offset(&mut out_offset, bytes_written)?;
        }

        splice_advance_offset(&mut in_offset, moved_from_chunk)?;
        total_moved += moved_from_chunk;
        left -= moved_from_chunk;

        if moved_from_chunk < bytes_read {
            if in_offset.is_none() {
                let unread = bytes_read - moved_from_chunk;
                if unread > 0 {
                    let _ = in_file.seek(-(unread as isize), SeekWhence::CUR);
                }
            }
            break;
        }
    }

    if !uptr_off_in.is_null() {
        uptr_off_in.write(in_offset.unwrap())?;
    }
    if !uptr_off_out.is_null() {
        uptr_off_out.write(out_offset.unwrap())?;
    }

    Ok(total_moved)
}

pub fn tee(in_fd: usize, out_fd: usize, len: usize, flags: usize) -> SyscallRet {
    let flags = SpliceFlags::from_bits(flags).ok_or(Errno::EINVAL)?;

    let mut fdtable = current::fdtable().lock();
    let in_file = fdtable.get(in_fd)?;
    let out_file = fdtable.get(out_fd)?;
    drop(fdtable);

    if !in_file.readable() || !out_file.writable() {
        return Err(Errno::EBADF);
    }

    let in_pipe = in_file.downcast_ref::<Pipe>().ok_or(Errno::EINVAL)?;
    let out_pipe = out_file.downcast_ref::<Pipe>().ok_or(Errno::EINVAL)?;

    if in_pipe.is_same_pipe(out_pipe) {
        return Err(Errno::EINVAL);
    }

    if len == 0 {
        return Ok(0);
    }

    let blocked = !flags.contains(SpliceFlags::NONBLOCK);
    let buffer = in_pipe.peek_with_blocked(len, blocked)?;
    if buffer.is_empty() {
        return Ok(0);
    }

    let mut total_moved = 0usize;
    while total_moved < buffer.len() {
        let end = core::cmp::min(total_moved + BUFFER_SIZE, buffer.len());
        let bytes_written = match out_pipe.write_with_blocked(&buffer[total_moved..end], blocked) {
            Ok(n) => n,
            Err(e) => {
                if total_moved > 0 {
                    break;
                }
                return Err(e);
            }
        };

        if bytes_written == 0 {
            break;
        }

        total_moved += bytes_written;

        if bytes_written < end - total_moved + bytes_written {
            break;
        }
    }

    Ok(total_moved)
}

#[repr(usize)]
#[derive(TryFromPrimitive)]
enum IOCTLReq {
    FIOCLEX = 0x5451,
}

pub fn ioctl(fd: usize, request: usize, arg: usize) -> SyscallRet {
    let file = current::fdtable().lock().get(fd)?;

    if let Some(req) = IOCTLReq::try_from(request).ok() {
        match req {
            IOCTLReq::FIOCLEX => {
                current::fdtable().lock().set_fd_flags(fd, FDFlags { cloexec: true })?;
                Ok(0)
            }
        }
    } else {
        file.ioctl(request, arg, &current::addrspace())
    }
}

pub fn faccessat(dirfd: usize, uptr_path: UString, _mode: usize) -> SyscallRet {
    uptr_path.should_not_null()?;

    let path = uptr_path.read_fixed()?;

    if dirfd as isize == AT_FDCWD {
        current::with_cwd(|cwd| vfs::load_dentry_at(&cwd, &path))?;
    } else {
        let file = current::fdtable().lock().get(dirfd)?;
        vfs::load_dentry_at(file.get_dentry().ok_or(Errno::ENOTDIR)?, &path)?;
    }

    Ok(0)
}

pub fn faccessat2(dirfd: usize, uptr_path: UString, _mode: usize, _flags: usize) -> SyscallRet {
    uptr_path.should_not_null()?;

    let path = uptr_path.read_fixed()?;

    if dirfd as isize == AT_FDCWD {
        current::with_cwd(|cwd| vfs::load_dentry_at(&cwd, &path))?;
    } else {
        let file = current::fdtable().lock().get(dirfd)?;
        vfs::load_dentry_at(file.get_dentry().ok_or(Errno::ENOTDIR)?, &path)?;
    }

    Ok(0)
}

bitflags! {
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub struct AtFlags: usize {
        const AT_SYMLINK_NOFOLLOW = 0x100;
        const AT_EMPTY_PATH = 0x1000;
    }
}

pub fn fstatat(dirfd: usize, uptr_path: UString, uptr_stat: UPtr<FileStat>, flags: usize) -> SyscallRet {
    uptr_stat.should_not_null()?;

    let flags = AtFlags::from_bits(flags).ok_or(Errno::EINVAL)?;

    let path = if flags.contains(AtFlags::AT_EMPTY_PATH) {
        str256::new()
    } else {
        uptr_path.should_not_null()?;
        uptr_path.read_fixed()?
    };

    let fstat = if path.is_empty() {
        current::fdtable().lock().get(dirfd)?.fstat()?
    } else {
        let helper = if flags.contains(AtFlags::AT_SYMLINK_NOFOLLOW) {
            vfs::load_dentry_at_nofollow
        } else {
            vfs::load_dentry_at
        };
        let dentry = if dirfd as isize == AT_FDCWD {
            current::with_cwd(|cwd| helper(&cwd, &path))
        } else {
            helper(
                current::fdtable()
                    .lock()
                    .get(dirfd)?
                    .get_dentry()
                    .ok_or(Errno::ENOTDIR)?,
                &path,
            )
        }?;

        dentry.get_inode().fstat()?
    };

    uptr_stat.write(fstat)?;

    Ok(0)
}

pub fn statfs64(uptr_path: UString, uptr_buf: UPtr<Statfs>) -> SyscallRet {
    uptr_path.should_not_null()?;
    uptr_buf.should_not_null()?;

    let path = uptr_path.read_fixed()?;
    let dentry = current::with_cwd(|cwd| vfs::load_dentry_at(&cwd, &path))?;

    let statfs = vfs::statfs(dentry.sno())?;

    uptr_buf.write(statfs)?;

    Ok(0)
}

pub fn newfstat(fd: usize, uptr_stat: UPtr<FileStat>) -> SyscallRet {
    let file = current::fdtable().lock().get(fd)?;

    let fstat = file.fstat()?;

    uptr_stat.write(fstat)?;

    Ok(0)
}

const UTIME_NOW: u64 = 0x3fffffff;
const UTIME_OMIT: u64 = 0x3ffffffe;

pub fn utimensat(dirfd: usize, uptr_path: UString, uptr_times: UArray<Timespec>, _flags: usize) -> SyscallRet {
    let path = if uptr_path.is_null() {
        str256::new()
    } else {
        uptr_path.read_fixed()?
    };
    let dentry = if dirfd as isize == AT_FDCWD {
        current::with_cwd(|cwd| vfs::load_dentry_at(&cwd, &path))?
    } else {
        vfs::load_dentry_at(
            current::fdtable()
                .lock()
                .get(dirfd)?
                .get_dentry()
                .ok_or(Errno::ENOTDIR)?,
            &path,
        )?
    };
    let inode = dentry.get_inode();

    let now = driver::chosen::kclock::now()?;

    if uptr_times.is_null() {
        inode.update_atime(&now)?;
        inode.update_mtime(&now)?;
        return Ok(0);
    }

    let atime = uptr_times.index(0).read()?;
    let mtime = uptr_times.index(1).read()?;
    if atime.tv_nsec != UTIME_OMIT {
        if atime.tv_nsec == UTIME_NOW {
            inode.update_atime(&now)?;
        } else {
            let duration = Duration::new(atime.tv_sec, atime.tv_nsec as u32);
            inode.update_atime(&duration)?;
        }
    }

    if mtime.tv_nsec != UTIME_OMIT {
        if mtime.tv_nsec == UTIME_NOW {
            inode.update_mtime(&now)?;
        } else {
            let duration = Duration::new(mtime.tv_sec, mtime.tv_nsec as u32);
            inode.update_mtime(&duration)?;
        }
    }

    Ok(0)
}

pub fn mkdirat(dirfd: usize, uptr_path: UString, mode: usize) -> SyscallRet {
    if mode > 0o7777 {
        return Err(Errno::EINVAL);
    }
    let mode = Mode::from_bits(mode as u32 & !current::umask()).ok_or(Errno::EINVAL)? | Mode::S_IFDIR;
    uptr_path.should_not_null()?;

    let path = uptr_path.read_fixed()?;

    let (parent, name) = if dirfd as isize == AT_FDCWD {
        current::with_cwd(|cwd| vfs::load_parent_dentry_at(&cwd, &path))?.ok_or(Errno::EEXIST)?
    } else {
        vfs::load_parent_dentry_at(
            current::fdtable()
                .lock()
                .get(dirfd)?
                .get_dentry()
                .ok_or(Errno::ENOTDIR)?,
            &path,
        )?
        .ok_or(Errno::EEXIST)?
    };

    parent.create(name, mode, Owner::new(current::fsuid(), current::fsgid()))?;

    Ok(0)
}

const DIRENT_NAME_OFFSET: usize = 8 + 8 + 2 + 1; // d_ino + d_off + d_reclen + d_type

pub fn getdents64(fd: usize, uptr_dirent: usize, count: usize) -> SyscallRet {
    let file = current::fdtable().lock().get(fd)?;
    let file = file.downcast_arc::<File>().map_err(|_| Errno::EBADF)?;

    if uptr_dirent == 0 {
        return Err(Errno::EINVAL);
    }

    let mut total_copied = 0;

    loop {
        let (dent, old_pos) = match file.get_dent() {
            Ok(Some(d)) => d,
            Ok(None) => {
                if total_copied == 0 {
                    return Ok(0); // No more entries
                } else {
                    break;
                }
            }
            Err(e) => return Err(e),
        };

        let name = &dent.name;
        let name_bytes = name.as_bytes();
        let name_len = core::cmp::min(name_bytes.len(), 255);
        let reclen = DIRENT_NAME_OFFSET + name_len + 1;
        let reclen_aligned = (reclen + 7) & !7; // Align to 8 bytes

        if total_copied + reclen_aligned > count {
            file.seek(old_pos as isize, SeekWhence::BEG)?; // Rewind one entry
            break;
        }

        let dirent = Dirent {
            d_ino: dent.ino as u64,
            d_off: 0, // Not used
            d_reclen: reclen_aligned as u16,
            d_type: DirentType::from(dent.file_type) as u8,
        };

        // Copy dirent to user space
        let dirent_ptr = uptr_dirent + total_copied;

        copy_to_user::object(dirent_ptr, dirent)?;
        copy_to_user::string(dirent_ptr + DIRENT_NAME_OFFSET, name, name_len + 1)?;

        total_copied += reclen_aligned;
    }

    if total_copied == 0 {
        Err(Errno::EINVAL)
    } else {
        Ok(total_copied)
    }
}

pub fn unlinkat(dirfd: usize, uptr_path: UString, _flags: usize) -> SyscallRet {
    uptr_path.should_not_null()?;

    let path = uptr_path.read_fixed()?;
    const AT_REMOVEDIR: usize = 0x200;

    if _flags & !AT_REMOVEDIR != 0 {
        return Err(Errno::EINVAL);
    }

    let parent_dentry = if path.starts_with('/') {
        vfs::load_parent_dentry(&path)?.ok_or(Errno::EOPNOTSUPP)
    } else if dirfd as isize == AT_FDCWD {
        current::with_cwd(|cwd| vfs::load_parent_dentry_at(&cwd, &path))?.ok_or(Errno::EOPNOTSUPP)
    } else {
        vfs::load_parent_dentry_at(
            current::fdtable()
                .lock()
                .get(dirfd)?
                .get_dentry()
                .ok_or(Errno::ENOTDIR)?,
            &path,
        )?
        .ok_or(Errno::EOPNOTSUPP)
    }?;

    let parent = parent_dentry.0;
    let name = &parent_dentry.1;

    if _flags == AT_REMOVEDIR {
        parent.rmdir(name)?;
    } else {
        parent.unlink(name)?;
    }

    Ok(0)
}

pub fn symlinkat(uptr_target: UString, newdirfd: usize, uptr_newname: UString) -> SyscallRet {
    uptr_target.should_not_null()?;
    uptr_newname.should_not_null()?;

    let target = uptr_target.read_fixed()?;
    let new_name = uptr_newname.read_fixed()?;
    if new_name.is_empty() {
        return Err(Errno::ENOENT);
    }

    let (parent, name) = if new_name.starts_with('/') {
        vfs::load_parent_dentry(&new_name)?.ok_or(Errno::EOPNOTSUPP)?
    } else if newdirfd as isize == AT_FDCWD {
        current::with_cwd(|cwd| vfs::load_parent_dentry_at(&cwd, &new_name))?.ok_or(Errno::EOPNOTSUPP)?
    } else {
        vfs::load_parent_dentry_at(
            current::fdtable()
                .lock()
                .get(newdirfd)?
                .get_dentry()
                .ok_or(Errno::ENOTDIR)?,
            &new_name,
        )?
        .ok_or(Errno::EOPNOTSUPP)?
    };

    parent.create_symlink(name, &target, Owner::new(current::fsuid(), current::fsgid()))?;

    Ok(0)
}

pub fn linkat(olddirfd: usize, uptr_oldpath: UString, newdirfd: usize, uptr_newpath: UString) -> SyscallRet {
    uptr_oldpath.should_not_null()?;
    uptr_newpath.should_not_null()?;

    let old_path = uptr_oldpath.read_fixed()?;
    let new_path = uptr_newpath.read_fixed()?;

    let old_dentry = if olddirfd as isize == AT_FDCWD {
        current::with_cwd(|cwd| vfs::load_dentry_at(&cwd, &old_path))
    } else {
        vfs::load_dentry_at(
            current::fdtable()
                .lock()
                .get(olddirfd)?
                .get_dentry()
                .ok_or(Errno::ENOTDIR)?,
            &old_path,
        )
    }?;

    let new_parent_dentry = if newdirfd as isize == AT_FDCWD {
        current::with_cwd(|cwd| vfs::load_parent_dentry_at(&cwd, &new_path))?.ok_or(Errno::EOPNOTSUPP)
    } else {
        vfs::load_parent_dentry(&new_path)?.ok_or(Errno::EOPNOTSUPP)
    }?;

    let new_parent = new_parent_dentry.0;
    let new_name = &new_parent_dentry.1;

    if old_dentry.sno() != new_parent.sno() {
        return Err(Errno::EXDEV); // Cross-device link
    }

    new_parent.link(new_name, &old_dentry)?;

    Ok(0)
}

bitflags! {
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub struct RenameFlags: usize {
        const RENAME_NOREPLACE = 1;
        const RENAME_EXCHANGE  = 2;
        const RENAME_WHITEOUT  = 4;
    }
}

pub fn renameat2(
    olddirfd: usize,
    uptr_oldpath: UString,
    newdirfd: usize,
    uptr_newpath: UString,
    _flags: usize,
) -> SysResult<usize> {
    uptr_oldpath.should_not_null()?;
    uptr_newpath.should_not_null()?;

    let old_path = uptr_oldpath.read_fixed()?;
    let new_path = uptr_newpath.read_fixed()?;

    let old_parent_dentry = if olddirfd as isize == AT_FDCWD {
        current::with_cwd(|cwd| vfs::load_parent_dentry_at(&cwd, &old_path))?.ok_or(Errno::EOPNOTSUPP)
    } else {
        vfs::load_parent_dentry(&old_path)?.ok_or(Errno::EOPNOTSUPP)
    }?;
    let new_parent_dentry = if newdirfd as isize == AT_FDCWD {
        current::with_cwd(|cwd| vfs::load_parent_dentry_at(&cwd, &new_path))?.ok_or(Errno::EOPNOTSUPP)
    } else {
        vfs::load_parent_dentry(&new_path)?.ok_or(Errno::EOPNOTSUPP)
    }?;

    let old_parent = old_parent_dentry.0;
    let old_name = old_parent_dentry.1;
    let new_parent = new_parent_dentry.0;
    let new_name = new_parent_dentry.1;

    if old_parent.sno() != new_parent.sno() {
        return Err(Errno::EXDEV); // Cross-device link
    }

    old_parent.rename(&old_name, &new_parent, &new_name)?;

    Ok(0)
}

pub fn fchmodat(dirfd: usize, uptr_path: UString, mode: usize) -> SyscallRet {
    let mut mode = Mode::from_bits(mode as u32 & 0o7777).ok_or(Errno::EINVAL)?;

    let path = uptr_path.should_not_null()?.read_fixed()?;
    if path.is_empty() {
        return Err(Errno::ENOENT);
    }

    let dentry = if dirfd as isize == AT_FDCWD {
        current::with_cwd(|cwd| vfs::load_dentry_at(&cwd, &path))?
    } else {
        vfs::load_dentry_at(
            current::fdtable()
                .lock()
                .get(dirfd)?
                .get_dentry()
                .ok_or(Errno::ENOTDIR)?,
            &path,
        )?
    };

    let inode = dentry.get_inode();
    if mode.contains(Mode::S_ISGID) && current::pcb().fsuid() != 0 {
        let inode_gid = inode.owner()?.1;
        let pcb = current::pcb();
        let in_supplementary_group = pcb.supplementary_gids().contains(&inode_gid);
        if pcb.fsgid() != inode_gid && !in_supplementary_group {
            mode.remove(Mode::S_ISGID);
        }
    }

    inode.chmod(mode)?;

    Ok(0)
}

pub fn fchmod(fd: usize, mode: usize) -> SyscallRet {
    let mut mode = Mode::from_bits(mode as u32 & 0o7777).ok_or(Errno::EINVAL)?;

    let file = current::fdtable().lock().get(fd)?;

    if let Some(inode) = file.get_dentry().and_then(|d| Some(d.get_inode())) {
        if mode.contains(Mode::S_ISGID) && current::pcb().fsuid() != 0 {
            let inode_gid = inode.owner()?.1;
            let pcb = current::pcb();
            let in_supplementary_group = pcb.supplementary_gids().contains(&inode_gid);
            if pcb.fsgid() != inode_gid && !in_supplementary_group {
                mode.remove(Mode::S_ISGID);
            }
        }
        inode.chmod(mode)?;
    } else {
        return Err(Errno::EINVAL);
    }

    Ok(0)
}

pub fn fchownat(dirfd: usize, uptr_path: UString, uid: usize, gid: usize, flags: usize) -> SyscallRet {
    let flags = AtFlags::from_bits(flags).ok_or(Errno::EINVAL)?;
    let path = if flags.contains(AtFlags::AT_EMPTY_PATH) {
        fixedstr::str256::new()
    } else {
        uptr_path.should_not_null()?;
        uptr_path.read_fixed()?
    };

    let dentry = if path.is_empty() {
        current::fdtable()
            .lock()
            .get(dirfd)?
            .get_dentry()
            .cloned()
            .ok_or(Errno::EINVAL)?
    } else {
        let helper = if flags.contains(AtFlags::AT_SYMLINK_NOFOLLOW) {
            vfs::load_dentry_at_nofollow
        } else {
            vfs::load_dentry_at
        };
        if dirfd as isize == AT_FDCWD {
            current::with_cwd(|cwd| helper(&cwd, &path))?
        } else {
            helper(
                current::fdtable()
                    .lock()
                    .get(dirfd)?
                    .get_dentry()
                    .ok_or(Errno::ENOTDIR)?,
                &path,
            )?
        }
    };

    let uid = uid as Uid;
    let gid = gid as Uid;

    let uid = if uid == Uid::MAX { None } else { Some(uid as Uid) };
    let gid = if gid == Uid::MAX { None } else { Some(gid as Uid) };
    dentry.get_inode().chown(uid, gid)?;

    Ok(0)
}

pub fn fchown(fd: usize, uid: usize, gid: usize) -> SyscallRet {
    let file = current::fdtable().lock().get(fd)?;

    let uid = uid as Uid;
    let gid = gid as Uid;

    if let Some(inode) = file.get_dentry().and_then(|d| Some(d.get_inode())) {
        let uid = if uid == Uid::MAX { None } else { Some(uid as Uid) };
        let gid = if gid == Uid::MAX { None } else { Some(gid as Uid) };
        inode.chown(uid, gid)?;
    } else {
        return Err(Errno::EINVAL);
    }

    Ok(0)
}

fn truncate_length(length: usize) -> SysResult<u64> {
    let length = length as i64;
    if length < 0 {
        return Err(Errno::EINVAL);
    }

    Ok(length as u64)
}

fn check_file_size_limit(length: u64) -> SysResult<()> {
    let (rlim_cur, _) = current::pcb().file_size_limit();
    if rlim_cur != usize::MAX && length > rlim_cur as u64 {
        let _ = current::pcb().send_signal(signum::SIGXFSZ, SiCode::SI_KERNEL, KSiFields::Empty, None);
        return Err(Errno::EFBIG);
    }

    Ok(())
}

pub fn truncate64(uptr_path: UString, length: usize) -> SyscallRet {
    let path = uptr_path.should_not_null()?.read_fixed()?;
    if path.is_empty() {
        return Err(Errno::ENOENT);
    }

    let length = truncate_length(length)?;
    let dentry = current::with_cwd(|cwd| vfs::load_dentry_at(&cwd, &path))?;
    let inode = dentry.get_inode();
    let mode = inode.mode()?;
    if mode.contains(Mode::S_IFDIR) {
        return Err(Errno::EISDIR);
    }

    check_file_size_limit(length)?;
    let (uid, gid) = inode.owner()?;
    if !mode.check_perm(&Perm::current(PermFlags::W), uid, gid) {
        return Err(Errno::EACCES);
    }

    inode.truncate(length)?;

    Ok(0)
}

pub fn ftruncate64(fd: usize, length: usize) -> SyscallRet {
    let file = current::fdtable().lock().get(fd)?;

    if !file.writable() {
        return Err(Errno::EBADF);
    }

    let length = truncate_length(length)?;
    check_file_size_limit(length)?;
    file.downcast_arc::<File>()
        .map_err(|_| Errno::EINVAL)?
        .ftruncate(length)?;

    Ok(0)
}

pub fn fallocate(fd: usize, mode: usize, _offset: usize, _len: usize) -> SyscallRet {
    let file = current::fdtable().lock().get(fd)?;

    if !file.writable() {
        return Err(Errno::EBADF);
    }

    if mode != 0 {
        return Err(Errno::EINVAL);
    }

    // file.downcast_arc::<File>()
    //     .map_err(|_| Errno::EINVAL)?
    //     .fallocate(offset as u64, len as u64)?;

    Ok(0)
}

pub fn umask(mask: usize) -> SyscallRet {
    if mask > 0o777 {
        return Err(Errno::EINVAL);
    }

    let pcb = current::pcb();
    let old_mask = pcb.umask();
    pcb.set_umask(mask as u16);

    Ok(old_mask as usize)
}

pub fn fsync(fd: usize) -> SyscallRet {
    let file = current::fdtable().lock().get(fd)?;

    file.fsync()?;

    Ok(0)
}

pub fn flock(fd: usize, _operation: usize) -> SyscallRet {
    let _file = current::fdtable().lock().get(fd)?;
    Ok(0)
}

pub fn mount(
    uptr_source: UString,
    uptr_target: UString,
    uptr_fstype: UString,
    _flags: usize,
    _data: usize,
) -> SyscallRet {
    use crate::fs::devfs::devnode::BlockDevInode;

    uptr_target.should_not_null()?;
    uptr_fstype.should_not_null()?;

    let target = uptr_target.read_fixed()?;
    let fstype = uptr_fstype.read_fixed()?;

    let device = if !uptr_source.is_null() {
        let source = uptr_source.read_fixed()?;
        // Resolve the source path to a block device inode
        let dentry = vfs::load_dentry(&source)?;
        let inode = dentry.get_inode();
        if let Ok(blk_inode) = inode.downcast_arc::<BlockDevInode>() {
            Some(blk_inode.driver().clone())
        } else {
            return Err(Errno::ENODEV);
        }
    } else {
        None
    };

    vfs::mount(&target, &fstype, device)?;

    Ok(0)
}

// TODO: Implement umount2 syscall
pub fn umount2(_uptr_target: UString, _flags: usize) -> SyscallRet {
    Err(Errno::ENOSYS)
}
