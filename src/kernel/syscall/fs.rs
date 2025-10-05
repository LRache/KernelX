use alloc::sync::Arc;
use bitflags::bitflags;
use num_enum::TryFromPrimitive;

use crate::fs::vfs::vfs;
use crate::kernel::errno::{Errno, SysResult};
use crate::kernel::scheduler::*;
use crate::kernel::task::fdtable::FDFlags;
use crate::fs::{vfs, Dentry};
use crate::fs::file::{File, FileFlags, FileOps, FileType, SeekWhence};
use crate::fs::inode::Mode;
use crate::{copy_from_user, copy_to_user, copy_to_user_string};
use crate::{kdebug, kinfo, ktrace};

use super::def::*;

pub fn dup(oldfd: usize) -> Result<usize, Errno> {
    let mut fdtable = current::fdtable().lock();
    let file = fdtable.get(oldfd)?;
    let newfd = fdtable.push(file.clone(), FDFlags::empty())?;
    Ok(newfd)
}

pub fn dup2(oldfd: usize, newfd: usize) -> Result<usize, Errno> {
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
enum FcntlCmd {
    F_DUPFD_CLOEXEC = 1030,
}

pub fn fcntl64(fd: usize, cmd: usize, _arg: usize) -> Result<usize, Errno> {
    match FcntlCmd::try_from(cmd).map_err(|_| Errno::EINVAL)? {
        FcntlCmd::F_DUPFD_CLOEXEC => {
            let mut fdtable = current::fdtable().lock();
            let file = fdtable.get(fd)?;
            let fd = fdtable.push(file, FDFlags { cloexec: true })?;
            Ok(fd)
        }
    }
}

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

pub fn openat(dirfd: usize, uptr_filename: usize, flags: usize, mode: usize) -> Result<usize, Errno> {
    let open_flags = OpenFlags::from_bits(flags).ok_or(Errno::EINVAL)?;
    let file_flags = FileFlags {
        writable: open_flags.contains(OpenFlags::O_WRONLY) || open_flags.contains(OpenFlags::O_RDWR),
        readable: open_flags.contains(OpenFlags::O_RDONLY) || open_flags.contains(OpenFlags::O_RDWR),
    };
    let fd_flags = FDFlags {
        cloexec: open_flags.contains(OpenFlags::O_CLOEXEC),
    };

    let path = current::get_user_string(uptr_filename)?;

    let helper = |parent: &Arc<Dentry>| {
        match vfs::openat_file(parent, &path, file_flags) {
            Ok(file) => Ok(file),
            Err(e) => {
                if e == Errno::ENOENT && open_flags.contains(OpenFlags::O_CREAT) {
                    // Create the file
                    if mode > 0o7777 {
                        return Err(Errno::EINVAL);
                    }
                    let mode = Mode::from_bits(mode as u16).ok_or(Errno::EINVAL)? | Mode::S_IFREG;
                    kinfo!("openat: creating file {} with mode {:#o}", path, mode);
                    let (parent_dentry, child_name) = vfs::openat_parent_dentry(parent, &path)?;
                    parent_dentry.create(&child_name, mode)?;                    
                    vfs::openat_file(parent, &path, file_flags)
                } else {
                    // kinfo!("openat: failed to open file {}: {:?}", path, e);
                    Err(e)
                }
            }
        }
    };

    let file = if dirfd as isize == AT_FDCWD {
        current::with_cwd(|cwd| helper(cwd))?
    } else {
        helper(vfs().get_root())?
    };

    let fd = current::fdtable().lock().push(Arc::new(file), fd_flags)?;

    kdebug!("openat called: path={}, flags={:?}, new_fd={}", path, open_flags, fd);

    Ok(fd)
}

pub fn read(fd: usize, user_buffer: usize, count: usize) -> Result<usize, Errno> {
    let file = current::fdtable().lock().get(fd)?;

    if !file.readable() {
        return Err(Errno::EBADF);
    }

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

        if bytes_read < to_read {
            break; // EOF
        }
    }

    // kinfo!("read: fd={}, requested={}, read={}, buf[0]={}", fd, count, total_read, buffer[0] as char);

    Ok(total_read)
}

pub fn readlinkat(dirfd: usize, uptr_path: usize, uptr_buf: usize, bufsize: usize) -> SysResult<usize> {
    let path = current::get_user_string(uptr_path)?;
    
    let path = if dirfd as isize == AT_FDCWD {
        current::with_cwd(|cwd| vfs::openat_dentry(cwd, &path, FileFlags::dontcare()))?
    } else {
        vfs::openat_dentry(
            current::fdtable().lock().get(dirfd)?.get_dentry().ok_or(Errno::ENOTDIR)?,
            &path, 
            FileFlags::dontcare()
        )?
    }.readlink()?;

    copy_to_user_string!(uptr_buf, &path, bufsize)
}

pub fn write(fd: usize, uptr_buffer: usize, mut count: usize) -> Result<usize, Errno> {
    let file = current::fdtable().lock().get(fd)?;
    if !file.writable() {
        return Err(Errno::EBADF);
    }

    let addrspace = current::addrspace();
    let mut written = 0;

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
    let file = current::fdtable().lock().get(fd)?;

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
    current::fdtable().lock().close(fd)?;

    Ok(0)
}

 pub fn sendfile(out_fd: usize, in_fd: usize, uptr_offset: usize, count: usize) -> Result<usize, Errno> {
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
    let mut local_offset = if uptr_offset != 0 {
        let t = 0;
        copy_from_user!(uptr_offset, t)?;
        t
    } else {
        in_file_offset
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

    if uptr_offset != 0 {
        copy_to_user!(uptr_offset, local_offset)?;
    } else {
        in_file.seek(local_offset as isize, SeekWhence::BEG)?;
    }

    Ok(total_sent)
}

pub fn ioctl(fd: usize, request: usize, arg: usize) -> Result<usize, Errno> {
    let file = current::fdtable().lock().get(fd)?;

    file.ioctl(request, arg)
}

// TODO: Implement faccessat
pub fn faccessat(_dirfd: usize, _uptr_path: usize, _mode: usize) -> Result<usize, Errno> {
    Ok(0)
}

pub fn fstatat(dirfd: usize, uptr_path: usize, uptr_statbuf: usize, _flags: usize) -> Result<usize, Errno> {
    let path = current::get_user_string(uptr_path)?;

    ktrace!("fstatat: path={}, dirfd={}", path, dirfd);

    let kstat = if dirfd as isize == AT_FDCWD {
        current::with_cwd(|cwd| vfs::openat_file(cwd, &path, FileFlags::dontcare()))?
    } else {
        vfs::openat_file(
            current::fdtable().lock().get(dirfd)?.get_dentry().ok_or(Errno::ENOTDIR)?,
            &path, 
            FileFlags::dontcare()
        )?
    }.fstat()?;

    copy_to_user!(uptr_statbuf, kstat)?;

    ktrace!("fstatat: path={}, st_size={}, st_mode={:#o}", path, kstat.st_size, kstat.st_mode);

    Ok(0)
}

pub fn newfstat(fd: usize, uptr_statbuf: usize) -> Result<usize, Errno> {
    let file = current::fdtable().lock().get(fd)?;

    let kstat = file.fstat()?;

    copy_to_user!(uptr_statbuf, kstat)?;

    ktrace!("newfstat: fd={}, st_size={}, st_mode={:#o}", fd, kstat.st_size, kstat.st_mode);

    Ok(0)
}

pub fn utimensat(_dirfd: usize, _uptr_path: usize, _uptr_times: usize, _flags: usize) -> Result<usize, Errno> {
    Err(Errno::ENOENT)
}

pub fn mkdirat(dirfd: usize, user_path: usize, mode: usize) -> Result<usize, Errno> {
    if mode > 0o7777 {
        return Err(Errno::EINVAL);
    }
    let mode = Mode::from_bits(mode as u16).ok_or(Errno::EINVAL)? | Mode::S_IFDIR;
    
    let path = current::get_user_string(user_path)?;

    let parent_dentry = if dirfd as isize == AT_FDCWD {
        current::with_cwd(|cwd| vfs::openat_parent_dentry(cwd, &path))?
    } else {
        vfs::open_parent_dentry(&path)?
    };

    let parent = parent_dentry.0;
    let name = &parent_dentry.1;

    parent.create(name, mode)?;

    Ok(0)
}

#[repr(C)]
struct Dirent {
    d_ino: u64,
    d_off: i64,
    d_reclen: u16,
    d_type: u8,
}

enum DirentType {
    Unknown     = 0,
    FIFO        = 1,
    CharDevice  = 2,
    Directory   = 4,
    BlockDevice = 6,
    RegularFile = 8,
    Symlink     = 10,
    Socket      = 12,
}

impl From<FileType> for DirentType {
    fn from(ft: FileType) -> Self {
        match ft {
            FileType::Regular     => DirentType::RegularFile,
            FileType::Directory   => DirentType::Directory,
            FileType::CharDevice  => DirentType::CharDevice,
            FileType::BlockDevice => DirentType::BlockDevice,
            FileType::FIFO        => DirentType::FIFO,
            FileType::Symlink     => DirentType::Symlink,
            FileType::Socket      => DirentType::Socket,
            FileType::Unknown     => DirentType::Unknown,
        }
    }
}

const DIRENT_NAME_OFFSET: usize = 8 + 8 + 2 + 1; // d_ino + d_off + d_reclen + d_type

pub fn getdents64(fd: usize, uptr_dirent: usize, count: usize) -> SysResult<usize> {
    let file = current::fdtable().lock().get(fd)?;

    kdebug!("getdents64: fd={}, buf={:#x}, count={}", fd, uptr_dirent, count);

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
                kdebug!("getdents64: error getting dent: {:?}", e);
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

        copy_to_user!(dirent_ptr, dirent).unwrap();
        kdebug!("getdents64: copied dirent d_ino={}, d_reclen={}, d_name={}", dirent.d_ino, dirent.d_reclen, name);
        copy_to_user_string!(dirent_ptr + DIRENT_NAME_OFFSET, name, name_len + 1)?;

        total_copied += reclen_aligned;
    }

    if total_copied == 0 {
        kdebug!("getdents64: no entries copied, buffer too small");
        Err(Errno::EINVAL)
    } else {
        Ok(total_copied)
    }
}
