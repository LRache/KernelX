use alloc::sync::Arc;
use bitflags::bitflags;
use num_enum::TryFromPrimitive;

use crate::kernel::errno::{Errno, SysResult};
use crate::kernel::scheduler::current::{copy_from_user, copy_to_user};
use crate::kernel::scheduler::*;
use crate::kernel::syscall::uptr::{UBuffer, UString, UserPointer};
use crate::kernel::syscall::SyscallRet;
use crate::kernel::task::fdtable::FDFlags;
use crate::kernel::uapi::{OpenFlags, Dirent, DirentType, FileStat};
use crate::fs::{Dentry, Mode};
use crate::fs::vfs;
use crate::fs::file::{File, FileFlags, FileOps, SeekWhence};
use crate::{kdebug, kinfo};

use super::def::*;
use super::uptr::UPtr;

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

pub fn fcntl64(fd: usize, cmd: usize, _arg: usize) -> SyscallRet {
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

        _ => Err(Errno::EINVAL),
    }
}

pub fn openat(dirfd: usize, uptr_filename: UString, flags: usize, mode: usize) -> SyscallRet {
    uptr_filename.should_not_null()?;
    
    let open_flags = OpenFlags::from_bits(flags).ok_or(Errno::EINVAL)?;
    let file_flags = FileFlags {
        writable: open_flags.contains(OpenFlags::O_WRONLY) || open_flags.contains(OpenFlags::O_RDWR),
        readable: open_flags.contains(OpenFlags::O_RDONLY) || open_flags.contains(OpenFlags::O_RDWR),
    };
    let fd_flags = FDFlags {
        cloexec: open_flags.contains(OpenFlags::O_CLOEXEC),
    };

    let path = uptr_filename.read()?;
    // kinfo!("openat: dirfd={}, path=\"{}\", flags={:?}, mode={:#o}", dirfd, path, open_flags, mode);

    let helper = |parent: &Arc<Dentry>| {
        kdebug!("parent={}", parent.get_path());
        match vfs::openat_file(parent, &path, file_flags) {
            Ok(file) => Ok(file),
            Err(e) => {
                if e == Errno::ENOENT && open_flags.contains(OpenFlags::O_CREAT) {
                    // Create the file
                    let mode = Mode::from_bits(mode as u16 & 0o777).ok_or(Errno::EINVAL)? | Mode::S_IFREG;
                    let (parent_dentry, child_name) = vfs::load_parent_dentry_at(parent, &path)?;
                    parent_dentry.create(&child_name, mode)?;                    
                    vfs::openat_file(parent, &path, file_flags)
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
    // let path = current::get_user_string(uptr_path)?;
    // let path = copy_from_user::string(uptr_path)?;
    uptr_path.should_not_null()?;
    ubuf.should_not_null()?;

    let path = uptr_path.read()?;
    
    let path = if dirfd as isize == AT_FDCWD {
        current::with_cwd(|cwd| vfs::load_dentry_at(cwd, &path))?
    } else {
        vfs::load_dentry_at(
            current::fdtable().lock().get(dirfd)?.get_dentry().ok_or(Errno::ENOTDIR)?,
            &path
        )?
    }.readlink()?;

    // copy_to_user_string!(uptr_buf, &path, bufsize)?;
    // current::copy_to_user_string(uptr_buf, &path, bufsize)?;
    // copy_to_user::string(uptr_buf, &path, bufsize)?;
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
        // addrspace.copy_from_user(uptr_buffer + written, &mut buffer[..to_write])
            // .map_err(|_| Errno::EFAULT)?;
        // copy_from_user::buffer(uptr_buffer + written, &mut buffer[..to_write])?;
        ubuf.read(written, &mut buffer[..to_write])?;
        
        file.write(&buffer[..to_write]).map_err(|_| Errno::EIO)?;

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

pub fn readv(fd: usize, uptr_iov: UPtr<IOVec>, iovcnt: usize) -> SyscallRet {
    let file = current::fdtable().lock().get(fd)?;

    if iovcnt == 0 {
        return Ok(0);
    }

    uptr_iov.should_not_null()?;

    let mut total_read = 0;

    for i in 0..iovcnt {
        let iov = uptr_iov.index(i).read()?;

        let mut read = 0usize;
        let mut remaining = iov.len;
        let mut buffer = [0u8; BUFFER_SIZE];
        while remaining != 0 {
            let to_read = core::cmp::min(remaining, BUFFER_SIZE);
            let bytes_read = file.read(&mut buffer[..to_read]).map_err(|_| Errno::EIO)?;
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

pub fn writev(fd: usize, uptr_iov: UPtr<IOVec>, iovcnt: usize) -> SyscallRet {
    let file = current::fdtable().lock().get(fd)?;

    if iovcnt == 0 {
        return Ok(0);
    }

    uptr_iov.should_not_null()?;

    let mut total_written = 0;

    for i in 0..iovcnt {
        // let mut iov_buf = [0u8; core::mem::size_of::<IOVec>()];
        // addrspace.copy_from_user(iov + i * core::mem::size_of::<IOVec>(), &mut iov_buf)
        //     .map_err(|_| Errno::EFAULT)?;
        // let iov = unsafe { &*(iov_buf.as_ptr() as *const IOVec) };
        // let iov: IOVec = copy_from_user::object(iov + i * core::mem::size_of::<IOVec>())?;
        let iov = uptr_iov.index(i).read()?;

        let mut written = 0usize;
        let mut remaining = iov.len;
        let mut buffer = [0u8; BUFFER_SIZE];
        while remaining != 0 {
            let to_write = core::cmp::min(remaining, BUFFER_SIZE);
            // addrspace.copy_from_user(iov.base + written, &mut buffer[..to_write])
            //     .map_err(|_| Errno::EFAULT)?;
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

    // if uptr_offset != 0 {
    //     // copy_to_user!(uptr_offset, local_offset)?;
    //     copy_to_user::object(uptr_offset, local_offset)?;
    // } else {
    //     in_file.seek(local_offset as isize, SeekWhence::BEG)?;
    // }
    if uptr_offset.is_null() {
        in_file.seek(local_offset as isize, SeekWhence::BEG)?;
    } else {
        uptr_offset.write(local_offset)?;
    }

    Ok(total_sent)
}

pub fn ioctl(fd: usize, request: usize, arg: usize) -> SyscallRet {
    let file = current::fdtable().lock().get(fd)?;

    file.ioctl(request, arg)
}

// TODO: Implement faccessat
pub fn faccessat(_dirfd: usize, _uptr_path: usize, _mode: usize) -> SyscallRet {
    Ok(0)
}

pub fn fstatat(dirfd: usize, uptr_path: UString, uptr_stat: UPtr<FileStat>, _flags: usize) -> SyscallRet {
    uptr_path.should_not_null()?;
    uptr_stat.should_not_null()?;
    
    let path = uptr_path.read()?;

    let fstat = if dirfd as isize == AT_FDCWD {
        current::with_cwd(|cwd| vfs::openat_file(cwd, &path, FileFlags::dontcare()))?
    } else {
        vfs::openat_file(
            current::fdtable().lock().get(dirfd)?.get_dentry().ok_or(Errno::ENOTDIR)?,
            &path, 
            FileFlags::dontcare()
        )?
    }.fstat()?;

    uptr_stat.write(fstat)?;

    Ok(0)
}

pub fn newfstat(fd: usize, uptr_stat: UPtr<FileStat>) -> SyscallRet {
    let file = current::fdtable().lock().get(fd)?;

    let fstat = file.fstat()?;

    // copy_to_user!(uptr_statbuf, kstat)?;
    // copy_to_user::object(uptr_statbuf, fstat)?;
    uptr_stat.write(fstat)?;

    // ktrace!("newfstat: fd={}, st_size={}, st_mode={:#o}", fd, fstat.st_size, fstat.st_mode);

    Ok(0)
}

pub fn utimensat(_dirfd: usize, _uptr_path: usize, _uptr_times: usize, _flags: usize) -> Result<usize, Errno> {
    Err(Errno::ENOENT)
}

pub fn mkdirat(dirfd: usize, uptr_path: UString, mode: usize) -> SyscallRet {
    if mode > 0o7777 {
        return Err(Errno::EINVAL);
    }
    let mode = Mode::from_bits(mode as u16).ok_or(Errno::EINVAL)? | Mode::S_IFDIR;

    uptr_path.should_not_null()?;
    
    // let path = current::get_user_string(user_path)?;
    // let path = copy_from_user::string(user_path)?;
    let path = uptr_path.read()?;

    let parent_dentry = if dirfd as isize == AT_FDCWD {
        current::with_cwd(|cwd| vfs::load_parent_dentry_at(cwd, &path))?
    } else {
        vfs::load_parent_dentry(&path)?
    };

    let parent = parent_dentry.0;
    let name = &parent_dentry.1;

    parent.create(name, mode)?;

    Ok(0)
}

const DIRENT_NAME_OFFSET: usize = 8 + 8 + 2 + 1; // d_ino + d_off + d_reclen + d_type

pub fn getdents64(fd: usize, uptr_dirent: usize, count: usize) -> SyscallRet {
    let file = current::fdtable().lock().get(fd)?;

    if uptr_dirent == 0 {
        return Err(Errno::EINVAL);
    }

    let mut total_copied = 0;
    
    loop {
        let dent = match file.get_dent() {
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
        kdebug!("getdents64: dent ino={}, name={}, reclen_aligned={}", dent.ino, name, reclen_aligned);

        if total_copied + reclen_aligned > count {
            kdebug!("getdents64: buffer full, total_copied={}, reclen_aligned={}, count={}", total_copied, reclen_aligned, count);
            file.seek(-1, SeekWhence::CUR)?; // Rewind one entry
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

        // copy_to_user!(dirent_ptr, dirent).unwrap();
        // copy_to_user_string!(dirent_ptr + DIRENT_NAME_OFFSET, name, name_len + 1)?;
        copy_to_user::object(dirent_ptr, dirent)?;
        copy_to_user::string(dirent_ptr + DIRENT_NAME_OFFSET, name, name_len + 1)?;

        total_copied += reclen_aligned;
    }

    if total_copied == 0 {
        kdebug!("getdents64: no entries copied, buffer too small");
        Err(Errno::EINVAL)
    } else {
        Ok(total_copied)
    }
}

pub fn unlinkat(dirfd: usize, uptr_path: UString, _flags: usize) -> SyscallRet {
    uptr_path.should_not_null()?;
    
    let path = uptr_path.read()?;

    let parent_dentry = if dirfd as isize == AT_FDCWD {
        current::with_cwd(|cwd| vfs::load_parent_dentry_at(cwd, &path))?
    } else {
        vfs::load_parent_dentry(&path)?
    };

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
        current::with_cwd(|cwd| vfs::load_parent_dentry_at(cwd, &old_path))?
    } else {
        vfs::load_parent_dentry(&old_path)?
    };
    let new_parent_dentry = if newdirfd as isize == AT_FDCWD {
        current::with_cwd(|cwd| vfs::load_parent_dentry_at(cwd, &new_path))?
    } else {
        vfs::load_parent_dentry(&new_path)?
    };

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
