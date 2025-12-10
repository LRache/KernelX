use core::usize;
use core::time::Duration;

use alloc::sync::Arc;
use bitflags::bitflags;
use num_enum::TryFromPrimitive;

use crate::arch::get_time_us;
use crate::kernel::errno::{Errno, SysResult};
use crate::kernel::scheduler::current::{copy_from_user, copy_to_user};
use crate::kernel::scheduler::*;
use crate::kernel::syscall::uptr::{UserPointer, UArray, UBuffer, UString, UPtr};
use crate::kernel::syscall::{SyscallRet, UserStruct};
use crate::kernel::task::fdtable::FDFlags;
use crate::kernel::uapi::{Dirent, DirentType, FileStat, OpenFlags, Statfs, Timespec};
use crate::fs::{Dentry, Mode, Perm, PermFlags};
use crate::fs::vfs;
use crate::fs::file::{File, FileFlags, FileOps, SeekWhence};

use super::def::*;

pub fn dup(oldfd: usize) -> SyscallRet {
    let mut fdtable = current::fdtable().lock();
    let file = fdtable.get(oldfd)?;
    let newfd = fdtable.push(file.clone(), FDFlags::empty())?;
    Ok(newfd)
}

pub fn dup2(oldfd: usize, newfd: usize) -> SyscallRet {
    let mut fdtable = current::fdtable().lock();
    if oldfd == newfd {
        // If oldfd and newfd are the same, just return newfd
        fdtable.get(oldfd)?; // Check if oldfd is valid
        return Ok(newfd);
    }

    let file = fdtable.get(oldfd)?;
    fdtable.set(newfd, file, FDFlags::empty())?;
    
    Ok(newfd)
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
}

bitflags! {
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub struct FDArgs: usize {
        const FD_CLOEXEC = 1;
    }
}

pub fn fcntl64(fd: usize, cmd: usize, arg: usize) -> SyscallRet {
    match FcntlCmd::try_from(cmd).map_err(|_| Errno::EINVAL)? {
        FcntlCmd::F_DUPFD_CLOEXEC => {
            let mut fdtable = current::fdtable().lock();
            let file = fdtable.get(fd)?;
            let fd = fdtable.push(file, FDFlags { cloexec: true })?;
            Ok(fd)
        }

        FcntlCmd::F_GETFL => {
            let file = current::fdtable().lock().get(fd)?;
            let mut open_flags = OpenFlags::O_RDONLY;
            if file.readable() && file.writable() {
                open_flags = OpenFlags::O_RDWR;
            } else if file.writable() {
                open_flags = OpenFlags::O_WRONLY;
            }
            Ok(open_flags.bits())
        }

        FcntlCmd::F_SETFL => {
            let file = current::fdtable().lock().get(fd)?;
            let flags = FDArgs::from_bits(arg).ok_or(Errno::EINVAL)?;
            current::fdtable().lock().set(fd, file, FDFlags {
                cloexec: flags.contains(FDArgs::FD_CLOEXEC),
            })?;
            
            Ok(0)
        }

        FcntlCmd::F_SETFD => {
            let flags = FDArgs::from_bits(arg).ok_or(Errno::EINVAL)?;
            
            let mut fdtable = current::fdtable().lock();
            let mut fdflags = fdtable.get_fd_flags(fd)?;
            fdflags.cloexec = flags.contains(FDArgs::FD_CLOEXEC);
            fdtable.set_fd_flags(fd, fdflags)?;
            
            Ok(0)
        }

        _ => Err(Errno::EINVAL),
    }
}

pub fn openat(dirfd: usize, uptr_filename: UString, flags: usize, mode: usize) -> SyscallRet {
    uptr_filename.should_not_null()?;

    let open_flags = OpenFlags::from_bits(flags).ok_or(Errno::EINVAL)?;
    let readable = open_flags.contains(OpenFlags::O_RDONLY) || open_flags.contains(OpenFlags::O_RDWR);
    let writable = open_flags.contains(OpenFlags::O_WRONLY) || open_flags.contains(OpenFlags::O_RDWR);
    let file_flags = FileFlags {
        writable,
        readable,
        blocked: !open_flags.contains(OpenFlags::O_NONBLOCK)
    };
    let fd_flags = FDFlags {
        cloexec: open_flags.contains(OpenFlags::O_CLOEXEC),
    };

    let path = uptr_filename.read()?;
    
    let helper = |parent: &Arc<Dentry>| {
        if open_flags.contains(OpenFlags::O_TMPFILE) {
            if !open_flags.contains(OpenFlags::O_WRONLY) || open_flags.contains(OpenFlags::O_RDWR) {
                return Err(Errno::EINVAL)
            }

            let dentry = vfs::load_dentry_at(parent, &path)?;
            return vfs::create_temp(&dentry, file_flags, Mode::from_bits(mode as u32 & 0o777).ok_or(Errno::EINVAL)? | Mode::S_IFREG);
        }

        let mut perm_flags = PermFlags::empty();
        if readable { perm_flags.insert(PermFlags::R); }
        if writable { perm_flags.insert(PermFlags::W); }
        
        match vfs::openat_file(parent, &path, file_flags, &Perm::new(perm_flags)) {
            Ok(file) => {
                if open_flags.contains(OpenFlags::O_CREATE) && open_flags.contains(OpenFlags::O_EXCL) {
                    return Err(Errno::EEXIST);
                }
                Ok(file)
            }
            Err(e) => {
                if e == Errno::ENOENT && open_flags.contains(OpenFlags::O_CREATE) {
                    // Create the file
                    let mode = Mode::from_bits(mode as u32 & 0o777 & !current::umask()).ok_or(Errno::EINVAL)? | Mode::S_IFREG;
                    let (parent_dentry, child_name) = vfs::load_parent_dentry_at(parent, &path)?.unwrap(); // SAFETY: The root must exist
                    parent_dentry.create(&child_name, mode)?;
                    vfs::openat_file(parent, &path, file_flags, &Perm::new(perm_flags))
                } else {
                    Err(e)
                }
            }
        }
    };

    let file = if dirfd as isize == AT_FDCWD {
        current::with_cwd(|cwd| helper(cwd))?
    } else {
        helper(vfs::get_root_dentry())?
    };

    let fd = current::fdtable().lock().push(Arc::new(file), fd_flags)?;

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
        
    let mut buffer = [0u8; BUFFER_SIZE];
    let mut total_read = 0;
    let mut left = count;

    while left > 0 {
        let to_read = core::cmp::min(left, BUFFER_SIZE);
        let bytes_read = file.read(&mut buffer[..to_read])?;
        if bytes_read == 0 {
            break; // EOF
        }
        
        ubuf.write(total_read, &buffer[..bytes_read])?;

        total_read += bytes_read;
        left -= bytes_read;

        if bytes_read < to_read {
            break; // EOF
        }
    }

    Ok(total_read)
}

pub fn readlinkat(dirfd: usize, uptr_path: UString, ubuf: UString, bufsize: usize) -> SyscallRet {
    uptr_path.should_not_null()?;
    ubuf.should_not_null()?;

    let path = uptr_path.read()?;
    
    // TODO: Implement /proc/self/exe properly
    if path == "/proc/self/exe" {
        let exe_path = current::pcb().exec_path();
        return ubuf.write(&exe_path, bufsize)
    }

    crate::kinfo!("readlinkat: dirfd={}, path=\"{}\"", dirfd as isize, path);
    
    let path = if dirfd as isize == AT_FDCWD {
        current::with_cwd(|cwd| vfs::load_dentry_at(cwd, &path))?
    } else {
        vfs::load_dentry_at(
            current::fdtable().lock().get(dirfd)?.get_dentry().ok_or(Errno::ENOTDIR)?,
            &path
        )?
    }.readlink()?;

    ubuf.write(&path, bufsize)?;

    Ok(0)
}

pub fn write(fd: usize, ubuf: UBuffer, mut count: usize) -> SyscallRet {
    if count == 0 {
        return Ok(0);
    }
    
    ubuf.should_not_null()?;
    
    let file = current::fdtable().lock().get(fd)?;
    if !file.writable() {
        return Err(Errno::EBADF);
    }

    let mut written = 0;

    let mut buffer = [0u8; BUFFER_SIZE];
    while count != 0 {
        let to_write = core::cmp::min(count, BUFFER_SIZE);
        ubuf.read(written, &mut buffer[..to_write])?;
        
        file.write(&buffer[..to_write])?;

        count -= to_write;
        written += to_write;
    }
    
    Ok(written)
}

#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct IOVec {
    pub base: usize,
    pub len: usize,
}

impl UserStruct for IOVec {}

pub fn readv(fd: usize, uptr_iov: UPtr<IOVec>, iovcnt: usize) -> SyscallRet {
    let file = current::fdtable().lock().get(fd)?;

    if iovcnt == 0 {
        return Ok(0);
    }

    uptr_iov.should_not_null()?;

    let mut total_read = 0;

    for i in 0..iovcnt {
        let iov = uptr_iov.add(i).read()?;

        let mut read = 0usize;
        let mut remaining = iov.len;
        let mut buffer = [0u8; BUFFER_SIZE];
        while remaining != 0 {
            let to_read = core::cmp::min(remaining, BUFFER_SIZE);
            let bytes_read = file.read(&mut buffer[..to_read])?;
            if bytes_read == 0 {
                break; // EOF
            }
            
            copy_to_user::buffer(iov.base + read, &buffer[..bytes_read])?;

            remaining -= bytes_read;
            read += bytes_read;
        }

        total_read += read;
    }

    Ok(total_read)
}

pub fn pread64(fd: usize, ubuf: UBuffer, count: usize, pos: usize) -> SyscallRet {
    let file = current::fdtable().lock().get(fd)?;

    if count == 0 {
        return Ok(0)
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
    let file = current::fdtable().lock().get(fd)?;

    if count == 0 {
        return Ok(0)
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

pub fn writev(fd: usize, uptr_iov: UPtr<IOVec>, iovcnt: usize) -> SyscallRet {
    let file = current::fdtable().lock().get(fd)?;

    if iovcnt == 0 {
        return Ok(0);
    }

    uptr_iov.should_not_null()?;

    let mut total_written = 0;

    for i in 0..iovcnt {
        let iov = uptr_iov.add(i).read()?;

        let mut written = 0usize;
        let mut remaining = iov.len;
        let mut buffer = [0u8; BUFFER_SIZE];
        while remaining != 0 {
            let to_write = core::cmp::min(remaining, BUFFER_SIZE);
            copy_from_user::buffer(iov.base + written, &mut buffer[..to_write])
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
    current::fdtable().lock().close(fd)?;

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
        uptr_offset.read()?
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

pub fn ioctl(fd: usize, request: usize, arg: usize) -> SyscallRet {
    let file = current::fdtable().lock().get(fd)?;

    file.ioctl(request, arg, &current::addrspace())
}

pub fn faccessat(dirfd: usize, uptr_path: UString, _mode: usize) -> SyscallRet {
    uptr_path.should_not_null()?;

    let path = uptr_path.read()?;

    // crate::kinfo!("faccessat: dirfd={}, path=\"{}\"", dirfd, path);

    if dirfd as isize == AT_FDCWD {
        current::with_cwd(|cwd| vfs::load_dentry_at(cwd, &path))?;
    } else {
        let file = current::fdtable().lock().get(dirfd)?;
        vfs::load_dentry_at(file.get_dentry().ok_or(Errno::ENOTDIR)?, &path)?;
    }

    Ok(0)
}

pub fn fstatat(dirfd: usize, uptr_path: UString, uptr_stat: UPtr<FileStat>, _flags: usize) -> SyscallRet {
    uptr_path.should_not_null()?;
    // uptr_stat.should_not_null()?;
    
    let path = uptr_path.read()?;

    // crate::kinfo!("fstatat: dirfd={}, path=\"{}\"", dirfd, path);

    let fstat = if path.is_empty() {
        current::fdtable().lock().get(dirfd)?.fstat()?
    } else {
        if dirfd as isize == AT_FDCWD {
            current::with_cwd(|cwd| vfs::openat_file(cwd, &path, FileFlags::dontcare(), &Perm::dontcare()))?
        } else {
            vfs::openat_file(
                current::fdtable().lock().get(dirfd)?.get_dentry().ok_or(Errno::ENOTDIR)?,
                &path, 
                FileFlags::dontcare(),
                &Perm::dontcare()
            )?
        }.fstat()?
    };

    uptr_stat.write(fstat)?;

    Ok(0)
}

pub fn statfs64(uptr_path: UString, uptr_buf: UPtr<Statfs>) -> SyscallRet {
    uptr_path.should_not_null()?;
    uptr_buf.should_not_null()?;
    
    let path = uptr_path.read()?;
    let dentry = current::with_cwd(|cwd| vfs::load_dentry_at(cwd, &path))?;

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

const UTIME_NOW:  u64 = 0x3fffffff;
const UTIME_OMIT: u64 = 0x3ffffffe;

pub fn utimensat(dirfd: usize, uptr_path: UString, uptr_times: UArray<Timespec>, _flags: usize) -> SyscallRet {
    let path = if uptr_path.is_null() {
        ""
    } else {
        &uptr_path.read()?
    };
    let dentry = if dirfd as isize == AT_FDCWD {
        current::with_cwd(|cwd| vfs::load_dentry_at(cwd, &path))?
    } else {
        vfs::load_dentry_at(
            current::fdtable().lock().get(dirfd)?.get_dentry().ok_or(Errno::ENOTDIR)?,
            path
        )?
    };
    let inode = dentry.get_inode();
    
    let now = get_time_us();
    
    if uptr_times.is_null() {
        let duration = Duration::new((now / 1_000_000) as u64, (now % 1_000_000 * 1000) as u32);
        inode.update_atime(&duration)?;
        inode.update_mtime(&duration)?;
        return Ok(0);
    }
    
    let atime = uptr_times.index(0).read()?;
    let mtime = uptr_times.index(1).read()?;
    if atime.tv_nsec != UTIME_OMIT {
        if atime.tv_nsec == UTIME_NOW {
            let duration = Duration::new((now / 1_000_000) as u64, (now % 1_000_000 * 1000) as u32);
            inode.update_atime(&duration)?;
        } else {
            let duration = Duration::new(atime.tv_sec, atime.tv_nsec as u32);
            inode.update_atime(&duration)?;
        }
    }

    if mtime.tv_nsec != UTIME_OMIT {
        if mtime.tv_nsec == UTIME_NOW {
            let duration = Duration::new((now / 1_000_000) as u64, (now % 1_000_000 * 1000) as u32);
            inode.update_mtime(&duration)?;
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
    
    let path = uptr_path.read()?;

    // crate::kinfo!("mkdirat: dirfd={}, path=\"{}\", mode=0o{:o}", dirfd as isize, path, mode.bits() & 0o7777);

    let (parent, name) = if dirfd as isize == AT_FDCWD {
        current::with_cwd(|cwd| vfs::load_parent_dentry_at(cwd, &path))?.ok_or(Errno::EEXIST)?
    } else {
        vfs::load_parent_dentry_at(
            current::fdtable().lock().get(dirfd)?.get_dentry().ok_or(Errno::ENOTDIR)?,
            &path
        )?.ok_or(Errno::EEXIST)?
    };

    parent.create(name, mode)?;

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
            },
            Err(e) => {
                return Err(e)
            },
        };
        
        let name = &dent.name;
        let name_bytes = name.as_bytes();
        let name_len = core::cmp::min(name_bytes.len(), 255);
        let reclen = DIRENT_NAME_OFFSET + name_len + 1;
        let reclen_aligned = (reclen + 7) & !7; // Align to 8 bytes

        if total_copied + reclen_aligned > count {
            // crate::kinfo!("getdents64: reached count limit total_copied={} reclen_aligned={} count={}", total_copied, reclen_aligned, count);
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
    
    let path = uptr_path.read()?;

    let parent_dentry = if dirfd as isize == AT_FDCWD {
        current::with_cwd(|cwd| vfs::load_parent_dentry_at(cwd, &path))?.ok_or(Errno::EOPNOTSUPP)
    } else {
        vfs::load_parent_dentry(&path)?.ok_or(Errno::EOPNOTSUPP)
    }?;

    let parent = parent_dentry.0;
    let name = &parent_dentry.1;

    parent.unlink(name)?;

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

pub fn renameat2(olddirfd: usize, uptr_oldpath: UString, newdirfd: usize, uptr_newpath: UString, _flags: usize) -> SysResult<usize> {
    uptr_oldpath.should_not_null()?;
    uptr_newpath.should_not_null()?;
    
    let old_path = uptr_oldpath.read()?;
    let new_path = uptr_newpath.read()?;

    let old_parent_dentry = if olddirfd as isize == AT_FDCWD {
        current::with_cwd(|cwd| vfs::load_parent_dentry_at(cwd, &old_path))?.ok_or(Errno::EOPNOTSUPP)
    } else {
        vfs::load_parent_dentry(&old_path)?.ok_or(Errno::EOPNOTSUPP)
    }?;
    let new_parent_dentry = if newdirfd as isize == AT_FDCWD {
        current::with_cwd(|cwd| vfs::load_parent_dentry_at(cwd, &new_path))?.ok_or(Errno::EOPNOTSUPP)
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
    if mode > 0o777 {
        return Err(Errno::EINVAL);
    }
    let mode = Mode::from_bits(mode as u32).ok_or(Errno::EINVAL)?;

    uptr_path.should_not_null()?;
    
    let path = uptr_path.read()?;

    let dentry = if dirfd as isize == AT_FDCWD {
        current::with_cwd(|cwd| vfs::load_dentry_at(cwd, &path))?
    } else {
        vfs::load_dentry_at(
            current::fdtable().lock().get(dirfd)?.get_dentry().ok_or(Errno::ENOTDIR)?,
            &path
        )?
    };

    dentry.get_inode().chmod(mode)?;

    Ok(0)
}

pub fn ftruncate64(fd: usize, length: usize) -> SyscallRet {
    let file = current::fdtable().lock().get(fd)?;

    if !file.writable() {
        return Err(Errno::EBADF);
    }

    file.downcast_arc::<File>()
        .map_err(|_| Errno::EINVAL)?
        .ftruncate(length as u64)?;

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
