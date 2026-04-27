use alloc::string::String;
use alloc::sync::Arc;
use bitflags::bitflags;
use core::mem::size_of;
use core::time::Duration;
use core::usize;
use num_enum::TryFromPrimitive;

use crate::driver;
use crate::fs::devfs::devnode::BlockDevInode;
use crate::fs::file::{FileFlags, FileOps, RandomAccessFile, SeekWhence};
use crate::fs::inode::{BsdFlockType, PosixFlock, PosixFlockType};
use crate::fs::{Dentry, FileType, InodeOps, Mode, MountOptions, Owner, Perm, PermFlags, vfs};
use crate::kernel::errno::{Errno, SysResult};
use crate::kernel::event::Event;
use crate::kernel::ipc::{KSiFields, Pipe, SiCode, signum};
use crate::kernel::scheduler::current::{copy_from_user, copy_to_user};
use crate::kernel::scheduler::*;
use crate::kernel::syscall::uptr::{UArray, UBuffer, UPtr, UString, UserPointer};
use crate::kernel::syscall::{SyscallRet, UserStruct, utils};
use crate::kernel::task::fdtable::FDFlags;
use crate::kernel::task::pidfd::PidFile;
use crate::kernel::uapi::{Dirent, DirentType, FileStat, OpenFlags, Statfs, Uid};

use super::common::Timespec;
use super::def::*;

pub fn dup(oldfd: usize) -> SyscallRet {
    let fdtable = current::fdtable();
    let mut fdtable = fdtable.lock();
    fdtable.dup2(oldfd, FDFlags::empty())
}

pub fn dup3(oldfd: usize, newfd: usize, flags: usize) -> SyscallRet {
    let flags = OpenFlags::from_bits(flags).ok_or(Errno::EINVAL)?;
    let fd_flags = FDFlags {
        cloexec: flags.contains(OpenFlags::O_CLOEXEC),
    };
    let fdtable = current::fdtable();
    let mut fdtable = fdtable.lock();
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
    F_GETLK = 5,
    F_SETLK = 6,
    F_SETLKW = 7,
    F_GETLK64 = 12,
    F_SETLK64 = 13,
    F_SETLKW64 = 14,
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

bitflags! {
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    struct OpenResolveFlags: u64 {
        const RESOLVE_NO_XDEV = 0x01;
        const RESOLVE_NO_MAGICLINKS = 0x02;
        const RESOLVE_NO_SYMLINKS = 0x04;
        const RESOLVE_BENEATH = 0x08;
        const RESOLVE_IN_ROOT = 0x10;
        const RESOLVE_CACHED = 0x20;
    }
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct Flock {
    l_type: i16,
    l_whence: i16,
    __pad0: i32,
    l_start: i64,
    l_len: i64,
    l_pid: i32,
    __pad1: i32,
}

impl UserStruct for Flock {}

#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct OpenHow {
    flags: u64,
    mode: u64,
    resolve: u64,
}

impl UserStruct for OpenHow {}

const OPEN_HOW_SIZE: usize = size_of::<OpenHow>();

#[derive(TryFromPrimitive)]
#[repr(i16)]
enum FcntlLockType {
    Read = 0,
    Write = 1,
    Unlock = 2,
}

#[derive(TryFromPrimitive)]
#[repr(i16)]
enum FcntlWhence {
    Set = 0,
    Cur = 1,
    End = 2,
}

fn fcntl_lock_inode(file: &Arc<dyn FileOps>) -> SysResult<&Arc<dyn InodeOps>> {
    let inode = file.get_inode().ok_or(Errno::EINVAL)?;
    if inode.lock_state().is_none() {
        return Err(Errno::EINVAL);
    }
    Ok(inode)
}

fn random_access_file(file: &Arc<dyn FileOps>) -> SysResult<&RandomAccessFile> {
    if let Some(file) = file.downcast_ref::<RandomAccessFile>() {
        return Ok(file);
    }
    if let Some(file) = file.downcast_ref::<PidFile>()
        && let Some(file) = file.random_access_file()
    {
        return Ok(file);
    }
    Err(Errno::ESPIPE)
}

fn normalize_posix_flock(
    file: &Arc<dyn FileOps>,
    inode: &Arc<dyn InodeOps>,
    flock: &Flock,
) -> SysResult<(Option<PosixFlockType>, i64, i64)> {
    let lock_type = match FcntlLockType::try_from(flock.l_type).map_err(|_| Errno::EINVAL)? {
        FcntlLockType::Read => {
            if !file.readable() {
                return Err(Errno::EBADF);
            }
            Some(PosixFlockType::Read)
        }
        FcntlLockType::Write => {
            if !file.writable() {
                return Err(Errno::EBADF);
            }
            Some(PosixFlockType::Write)
        }
        FcntlLockType::Unlock => None,
    };

    let base = match FcntlWhence::try_from(flock.l_whence).map_err(|_| Errno::EINVAL)? {
        FcntlWhence::Set => 0,
        FcntlWhence::Cur => {
            i64::try_from(random_access_file(file)?.seek(0, SeekWhence::CUR)?).map_err(|_| Errno::EINVAL)?
        }
        FcntlWhence::End => i64::try_from(inode.size()?).map_err(|_| Errno::EINVAL)?,
    };

    let mut start = base.checked_add(flock.l_start).ok_or(Errno::EINVAL)?;
    if start < 0 {
        return Err(Errno::EINVAL);
    }

    let len = if flock.l_len < 0 {
        let end = start;
        start = start.checked_add(flock.l_len).ok_or(Errno::EINVAL)?;
        if start < 0 {
            return Err(Errno::EINVAL);
        }
        end - start
    } else if flock.l_len == 0 {
        0
    } else {
        start.checked_add(flock.l_len).ok_or(Errno::EINVAL)?;
        flock.l_len
    };

    Ok((lock_type, start, len))
}

fn flock_from_conflict(conflict: PosixFlock) -> Flock {
    Flock {
        l_type: match conflict.lock_type {
            PosixFlockType::Read => FcntlLockType::Read as i16,
            PosixFlockType::Write => FcntlLockType::Write as i16,
        },
        l_whence: FcntlWhence::Set as i16,
        __pad0: 0,
        l_start: conflict.start,
        l_len: conflict.len,
        l_pid: conflict.owner,
        __pad1: 0,
    }
}

fn fcntl_getlk(file: &Arc<dyn FileOps>, arg: usize) -> SyscallRet {
    let uptr_lock: UPtr<Flock> = arg.into();
    let mut user_lock = uptr_lock.should_not_null()?.read()?;
    let inode = fcntl_lock_inode(file)?;
    let (lock_type, start, len) = normalize_posix_flock(file, &inode, &user_lock)?;
    let lock_type = lock_type.ok_or(Errno::EINVAL)?;

    let conflict = {
        let state = inode.lock_state().unwrap().lock();
        state.posix.get_conflict(current::pid(), lock_type, start, len)
    };

    if let Some(conflict) = conflict {
        user_lock = flock_from_conflict(conflict);
    } else {
        user_lock.l_type = FcntlLockType::Unlock as i16;
    }

    uptr_lock.write(user_lock)?;
    Ok(0)
}

fn fcntl_setlk(file: &Arc<dyn FileOps>, arg: usize, blocking: bool) -> SyscallRet {
    let uptr_lock: UPtr<Flock> = arg.into();
    let user_lock = uptr_lock.should_not_null()?.read()?;
    let inode = fcntl_lock_inode(file)?;
    let (lock_type, start, len) = normalize_posix_flock(file, &inode, &user_lock)?;
    let owner = current::pid();
    let lock_state = inode.lock_state().unwrap();

    if lock_type.is_none() {
        let mut state = lock_state.lock();
        state.posix.apply(owner, None, start, len);
        state.posix.wake_all();
        return Ok(0);
    }

    let request_type = lock_type.unwrap();
    loop {
        let mut state = lock_state.lock();
        if state.posix.get_conflict(owner, request_type, start, len).is_none() {
            state.posix.apply(owner, Some(request_type), start, len);
            state.posix.wake_all();
            return Ok(0);
        }

        if !blocking {
            return Err(Errno::EAGAIN);
        }

        state.posix.wait_current();
        drop(state);

        current::schedule();
        match current::task().take_wakeup_event().unwrap() {
            Event::IOComplete => {}
            Event::Signal => return Err(Errno::EINTR),
            event => unreachable!("unexpected event while waiting on fcntl lock: {:?}", event),
        }
    }
}

pub fn fcntl64(fd: usize, cmd: usize, arg: usize) -> SyscallRet {
    match FcntlCmd::try_from(cmd).map_err(|_| Errno::EINVAL)? {
        FcntlCmd::F_DUPFD => {
            let fdtable = current::fdtable();
            let mut fdtable = fdtable.lock();
            fdtable.dup_min(fd, arg, FDFlags::empty())
        }

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
            let fdtable = current::fdtable();
            let fdtable = fdtable.lock();
            let fdflags = fdtable.get_fd_flags(fd)?;
            let mut flags = FDArgs::empty();
            if fdflags.cloexec {
                flags |= FDArgs::FD_CLOEXEC;
            }
            Ok(flags.bits())
        }

        FcntlCmd::F_SETFD => {
            let flags = FDArgs::from_bits(arg).ok_or(Errno::EINVAL)?;

            let fdtable = current::fdtable();
            let mut fdtable = fdtable.lock();
            let mut fdflags = fdtable.get_fd_flags(fd)?;
            fdflags.cloexec = flags.contains(FDArgs::FD_CLOEXEC);
            fdtable.set_fd_flags(fd, fdflags)?;

            Ok(0)
        }

        FcntlCmd::F_DUPFD_CLOEXEC => {
            let fdtable = current::fdtable();
            let mut fdtable = fdtable.lock();
            fdtable.dup_min(fd, arg, FDFlags { cloexec: true })
        }

        FcntlCmd::F_GETLK | FcntlCmd::F_GETLK64 => {
            let file = current::fdtable().lock().get(fd)?;
            fcntl_getlk(&file, arg)
        }

        FcntlCmd::F_SETLK | FcntlCmd::F_SETLK64 => {
            let file = current::fdtable().lock().get(fd)?;
            fcntl_setlk(&file, arg, false)
        }

        FcntlCmd::F_SETLKW | FcntlCmd::F_SETLKW64 => {
            let file = current::fdtable().lock().get(fd)?;
            fcntl_setlk(&file, arg, true)
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
    }
}

fn do_openat(dirfd: usize, path: String, flags: usize, mode: usize) -> SyscallRet {
    let open_flags = OpenFlags::from_bits(flags).ok_or(Errno::EINVAL)?;
    if open_flags.contains(OpenFlags::O_DIRECTORY) && open_flags.contains(OpenFlags::O_CREATE) {
        return Err(Errno::EINVAL);
    }
    if open_flags.contains(OpenFlags::O_NOATIME) && current::fsuid() != 0 {
        return Err(Errno::EPERM);
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

    if path.is_empty() {
        return Err(Errno::ENOENT);
    }

    let helper = |parent: &Arc<Dentry>| {
        if open_flags.contains(OpenFlags::O_TMPFILE) {
            if !writable {
                return Err(Errno::EINVAL);
            }

            let dentry = vfs::load_dentry_at(parent, &path)?;
            if dentry.is_superblock_readonly()? {
                return Err(Errno::EROFS);
            }
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

        let perm = Perm::current(perm_flags);
        let file = if open_flags.contains(OpenFlags::O_NOFOLLOW) {
            vfs::openat_file_nofollow(parent, &path, file_flags, &perm)
        } else {
            vfs::openat_file(parent, &path, file_flags, &perm)
        };

        match file {
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
                    let parent_dentry = parent_dentry.get_mount_to();
                    if parent_dentry.is_superblock_readonly()? {
                        return Err(Errno::EROFS);
                    }
                    vfs::create_file(
                        &parent_dentry,
                        child_name.as_ref(),
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

    let file = if path.starts_with('/') || dirfd as isize == AT_FDCWD {
        current::with_cwd(|cwd| helper(&cwd))?
    } else {
        let dir_file = current::fdtable().lock().get(dirfd)?;
        let dir = dir_file.get_dentry().ok_or(Errno::ENOTDIR)?;
        helper(dir)?
    };

    if open_flags.contains(OpenFlags::O_DIRECTORY) && !open_flags.contains(OpenFlags::O_TMPFILE) {
        let inode = file.get_inode().ok_or(Errno::ENOTDIR)?;
        if inode.inode_type()? != FileType::Directory {
            return Err(Errno::ENOTDIR);
        }
    }

    let fd = current::fdtable().lock().push(file.clone(), fd_flags)?;

    if writable && open_flags.contains(OpenFlags::O_TRUNC) {
        if let Some(inode) = file.get_inode()
            && inode.inode_type()? == FileType::Regular
            && let Err(err) = inode.truncate(0)
        {
            let _ = current::fdtable().lock().take(fd);
            return Err(err);
        }
    }

    Ok(fd)
}

fn do_openat_with_lookup_flags(
    dirfd: usize,
    path: String,
    flags: usize,
    mode: usize,
    lookup_flags: vfs::LookupFlags,
) -> SyscallRet {
    let open_flags = OpenFlags::from_bits(flags).ok_or(Errno::EINVAL)?;
    if open_flags.contains(OpenFlags::O_DIRECTORY) && open_flags.contains(OpenFlags::O_CREATE) {
        return Err(Errno::EINVAL);
    }
    if open_flags.contains(OpenFlags::O_NOATIME) && current::fsuid() != 0 {
        return Err(Errno::EPERM);
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

    if path.is_empty() {
        return Err(Errno::ENOENT);
    }

    let helper = |parent: &Arc<Dentry>| {
        if open_flags.contains(OpenFlags::O_TMPFILE) {
            if !writable {
                return Err(Errno::EINVAL);
            }

            let dentry = vfs::load_dentry_at_with_flags(parent, &path, lookup_flags)?;
            if dentry.is_superblock_readonly()? {
                return Err(Errno::EROFS);
            }
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

        let perm = Perm::current(perm_flags);
        let file = if open_flags.contains(OpenFlags::O_NOFOLLOW) {
            vfs::openat_file_nofollow_with_lookup_flags(parent, &path, file_flags, &perm, lookup_flags)
        } else {
            vfs::openat_file_with_lookup_flags(parent, &path, file_flags, &perm, lookup_flags)
        };

        match file {
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
                    let (parent_dentry, child_name) =
                        vfs::load_parent_dentry_at_with_flags(parent, &path, lookup_flags)?.unwrap(); // SAFETY: The root must exist
                    if parent_dentry.is_superblock_readonly()? {
                        return Err(Errno::EROFS);
                    }
                    vfs::create_file(
                        &parent_dentry,
                        child_name.as_ref(),
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

    let file = if path.starts_with('/') || dirfd as isize == AT_FDCWD {
        current::with_cwd(|cwd| helper(&cwd))?
    } else {
        let dir_file = current::fdtable().lock().get(dirfd)?;
        let dir = dir_file.get_dentry().ok_or(Errno::ENOTDIR)?;
        helper(dir)?
    };

    if open_flags.contains(OpenFlags::O_DIRECTORY) && !open_flags.contains(OpenFlags::O_TMPFILE) {
        let inode = file.get_inode().ok_or(Errno::ENOTDIR)?;
        if inode.inode_type()? != FileType::Directory {
            return Err(Errno::ENOTDIR);
        }
    }

    let fd = current::fdtable().lock().push(file.clone(), fd_flags)?;

    if writable && open_flags.contains(OpenFlags::O_TRUNC) {
        if let Some(inode) = file.get_inode()
            && inode.inode_type()? == FileType::Regular
            && let Err(err) = inode.truncate(0)
        {
            let _ = current::fdtable().lock().take(fd);
            return Err(err);
        }
    }

    Ok(fd)
}

pub fn openat(dirfd: usize, uptr_filename: UString, flags: usize, mode: usize) -> SyscallRet {
    uptr_filename.should_not_null()?;
    let path = uptr_filename.read_path()?;
    do_openat(dirfd, path, flags, mode)
}

fn read_open_how(uptr_how: UPtr<OpenHow>, size: usize) -> SysResult<OpenHow> {
    if size < OPEN_HOW_SIZE {
        return Err(Errno::EINVAL);
    }

    let how = uptr_how.read()?;
    if size > OPEN_HOW_SIZE {
        let extra_start = uptr_how.uaddr().checked_add(OPEN_HOW_SIZE).ok_or(Errno::EINVAL)?;
        let extra_len = size - OPEN_HOW_SIZE;
        let mut extra = alloc::vec![0u8; extra_len];
        copy_from_user::slice(extra_start, &mut extra)?;
        if extra.iter().any(|&byte| byte != 0) {
            return Err(Errno::E2BIG);
        }
    }

    Ok(how)
}

pub fn openat2(dirfd: usize, uptr_filename: UString, uptr_how: UPtr<OpenHow>, size: usize) -> SyscallRet {
    let how = read_open_how(uptr_how, size)?;
    let flags = usize::try_from(how.flags).map_err(|_| Errno::EINVAL)?;
    let mode = usize::try_from(how.mode).map_err(|_| Errno::EINVAL)?;
    let open_flags = OpenFlags::from_bits(flags).ok_or(Errno::EINVAL)?;
    let resolve_flags = OpenResolveFlags::from_bits(how.resolve).ok_or(Errno::EINVAL)?;
    if !OpenResolveFlags::RESOLVE_NO_XDEV.contains(resolve_flags) {
        // return Err(Errno::EINVAL);
    }
    if !open_flags.intersects(OpenFlags::O_CREATE | OpenFlags::O_TMPFILE) && mode != 0 {
        return Err(Errno::EINVAL);
    }
    if how.mode & !0o7777 != 0 {
        return Err(Errno::EINVAL);
    }

    let path = uptr_filename.read_path()?;
    if resolve_flags.is_empty() {
        do_openat(dirfd, path, flags, mode)
    } else {
        do_openat_with_lookup_flags(dirfd, path, flags, mode, vfs::LookupFlags::NO_XDEV)
    }
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

    let path = uptr_path.read_path()?;

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
        if let Some(size) = parent.readlink(child.as_ref(), &mut buffer)? {
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
    random_access_file(file).map(|_| ())
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
    let file = random_access_file(&file)?;
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

    let file = random_access_file(&file)?;
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
    let file = random_access_file(&file)?;
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
    let file = random_access_file(&file)?;
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
    let file = random_access_file(&file)?;
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
        current::tcb().unshare_fdtable()?;
    }

    let fdtable = current::fdtable();
    let mut fdtable = fdtable.lock();
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
    let fdtable = current::fdtable();
    let mut fdtable = fdtable.lock();
    let out_file = fdtable.get(out_fd)?;
    let in_file = fdtable
        .get(in_fd)?
        .downcast_arc::<RandomAccessFile>()
        .map_err(|_| Errno::EINVAL)?;
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
        Some(pos) => random_access_file(file)?.pread(buf, pos),
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
        Some(pos) => random_access_file(file)?.pwrite(buf, pos),
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

    let fdtable = current::fdtable();
    let mut fdtable = fdtable.lock();
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
                    if let Ok(file) = random_access_file(&in_file) {
                        let _ = file.seek(-(unread as isize), SeekWhence::CUR);
                    }
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

    let fdtable = current::fdtable();
    let mut fdtable = fdtable.lock();
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

bitflags! {
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    struct AccessMode: usize {
        const X_OK = 0x1;
        const W_OK = 0x2;
        const R_OK = 0x4;
    }
}

bitflags! {
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    struct AccessAtFlags: usize {
        const AT_SYMLINK_NOFOLLOW = 0x100;
        const AT_EACCESS = 0x200;
        const AT_EMPTY_PATH = 0x1000;
    }
}

fn access_perm_flags(mode: AccessMode) -> PermFlags {
    let mut perm = PermFlags::empty();
    if mode.contains(AccessMode::R_OK) {
        perm.insert(PermFlags::R);
    }
    if mode.contains(AccessMode::W_OK) {
        perm.insert(PermFlags::W);
    }
    if mode.contains(AccessMode::X_OK) {
        perm.insert(PermFlags::X);
    }
    perm
}

fn lookup_access_dentry(dirfd: usize, path: &str, flags: AccessAtFlags, search_perm: &Perm) -> SysResult<Arc<Dentry>> {
    if path.is_empty() {
        if !flags.contains(AccessAtFlags::AT_EMPTY_PATH) {
            return Err(Errno::ENOENT);
        }

        let dentry = if dirfd as isize == AT_FDCWD {
            current::with_cwd(|cwd| Ok(cwd))?
        } else {
            let file = current::fdtable().lock().get(dirfd)?;
            file.get_dentry().ok_or(Errno::ENOTDIR)?.clone()
        };
        return Ok(dentry.get_mount_to());
    }

    let helper = if flags.contains(AccessAtFlags::AT_SYMLINK_NOFOLLOW) {
        vfs::load_dentry_at_nofollow_with_perm
    } else {
        vfs::load_dentry_at_with_perm
    };

    let dentry = if path.starts_with('/') || dirfd as isize == AT_FDCWD {
        current::with_cwd(|cwd| helper(&cwd, path, search_perm))?
    } else {
        let file = current::fdtable().lock().get(dirfd)?;
        helper(file.get_dentry().ok_or(Errno::ENOTDIR)?, path, search_perm)?
    };

    Ok(dentry.get_mount_to())
}

fn last_path_component(path: &str) -> Option<&str> {
    let trimmed = path.trim_end_matches('/');
    if trimmed.is_empty() {
        None
    } else {
        trimmed.rsplit('/').next()
    }
}

fn do_faccessat(dirfd: usize, uptr_path: UString, mode: usize, flags: AccessAtFlags) -> SyscallRet {
    uptr_path.should_not_null()?;

    let mode = AccessMode::from_bits(mode).ok_or(Errno::EINVAL)?;
    let path = uptr_path.read_path()?;
    let search_perm = Perm::access(PermFlags::X, flags.contains(AccessAtFlags::AT_EACCESS));
    let perm = Perm::access(access_perm_flags(mode), flags.contains(AccessAtFlags::AT_EACCESS));
    let dentry = lookup_access_dentry(dirfd, &path, flags, &search_perm)?;

    if !mode.is_empty() {
        if mode.contains(AccessMode::W_OK) && dentry.is_superblock_readonly()? {
            return Err(Errno::EROFS);
        }

        let inode = dentry.get_inode();
        let mode = inode.mode()?;
        let (uid, gid) = inode.owner()?;
        if !mode.check_perm(&perm, uid, gid) {
            return Err(Errno::EACCES);
        }
    }

    Ok(0)
}

pub fn faccessat(dirfd: usize, uptr_path: UString, mode: usize) -> SyscallRet {
    do_faccessat(dirfd, uptr_path, mode, AccessAtFlags::empty())
}

pub fn faccessat2(dirfd: usize, uptr_path: UString, mode: usize, flags: usize) -> SyscallRet {
    let flags = AccessAtFlags::from_bits(flags).ok_or(Errno::EINVAL)?;
    do_faccessat(dirfd, uptr_path, mode, flags)
}

bitflags! {
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub struct AtFlags: usize {
        const AT_SYMLINK_NOFOLLOW = 0x100;
        const AT_EMPTY_PATH = 0x1000;
    }
}

pub fn fstatat(dirfd: usize, uptr_path: UString, uptr_stat: UPtr<FileStat>, flags: usize) -> SyscallRet {
    let flags = AtFlags::from_bits(flags).ok_or(Errno::EINVAL)?;

    let path = if uptr_path.is_null() && flags.contains(AtFlags::AT_EMPTY_PATH) {
        String::new()
    } else {
        uptr_path.read_path()?
    };

    let fstat = if path.is_empty() {
        if !flags.contains(AtFlags::AT_EMPTY_PATH) {
            return Err(Errno::ENOENT);
        }

        if dirfd as isize == AT_FDCWD {
            current::with_cwd(|cwd| cwd.get_inode().fstat())?
        } else {
            current::fdtable().lock().get(dirfd)?.fstat()?
        }
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

    let path = uptr_path.read_path()?;
    if path.is_empty() {
        return Err(Errno::ENOENT);
    }
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
        String::new()
    } else {
        uptr_path.read_path()?
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

    let path = uptr_path.read_path()?;

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

    parent.create(name.as_ref(), mode, Owner::new(current::fsuid(), current::fsgid()))?;

    Ok(0)
}

const DIRENT_NAME_OFFSET: usize = 8 + 8 + 2 + 1; // d_ino + d_off + d_reclen + d_type

pub fn getdents64(fd: usize, uptr_dirent: usize, count: usize) -> SyscallRet {
    let file = current::fdtable().lock().get(fd)?;
    let file = random_access_file(&file)?;

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

bitflags! {
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    struct UnlinkAtFlags: usize {
        const AT_REMOVEDIR = 0x200;
    }
}

pub fn unlinkat(dirfd: usize, uptr_path: UString, flags: usize) -> SyscallRet {
    uptr_path.should_not_null()?;

    let flags = UnlinkAtFlags::from_bits(flags).ok_or(Errno::EINVAL)?;
    let path = uptr_path.read_path()?;
    if path.is_empty() {
        return Err(Errno::ENOENT);
    }
    if flags.contains(UnlinkAtFlags::AT_REMOVEDIR) && last_path_component(&path) == Some(".") {
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

    let parent = parent_dentry.0.get_mount_to();
    let name = parent_dentry.1;

    if parent.is_superblock_readonly()? {
        return Err(Errno::EROFS);
    }

    if flags.contains(UnlinkAtFlags::AT_REMOVEDIR) {
        parent.rmdir(name.as_ref())?;
    } else {
        parent.unlink(name.as_ref())?;
    }

    Ok(0)
}

pub fn symlinkat(uptr_target: UString, newdirfd: usize, uptr_newname: UString) -> SyscallRet {
    uptr_target.should_not_null()?;
    uptr_newname.should_not_null()?;

    let target = uptr_target.read_path()?;
    let new_name = uptr_newname.read_path()?;
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

    parent.create_symlink(name.as_ref(), &target, Owner::new(current::fsuid(), current::fsgid()))?;

    Ok(0)
}

pub fn linkat(olddirfd: usize, uptr_oldpath: UString, newdirfd: usize, uptr_newpath: UString) -> SyscallRet {
    uptr_oldpath.should_not_null()?;
    uptr_newpath.should_not_null()?;

    let old_path = uptr_oldpath.read_path()?;
    let new_path = uptr_newpath.read_path()?;

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
    let new_name = new_parent_dentry.1;

    if old_dentry.sno() != new_parent.sno() {
        return Err(Errno::EXDEV); // Cross-device link
    }

    new_parent.link(new_name.as_ref(), &old_dentry)?;

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

    let old_path = uptr_oldpath.read_path()?;
    let new_path = uptr_newpath.read_path()?;

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

    old_parent.rename(old_name.as_ref(), &new_parent, new_name.as_ref())?;

    Ok(0)
}

fn do_chmod(dentry: &Arc<Dentry>, mode: usize) -> SyscallRet {
    let dentry = dentry.clone().get_mount_to();
    let mut mode = Mode::from_bits(mode as u32 & 0o7777).ok_or(Errno::EINVAL)?;
    if dentry.is_superblock_readonly()? {
        return Err(Errno::EROFS);
    }

    let inode = dentry.get_inode();
    let (inode_uid, inode_gid) = inode.owner()?;
    let pcb = current::pcb();
    let fsuid = pcb.fsuid();
    if fsuid != 0 && fsuid != inode_uid {
        return Err(Errno::EPERM);
    }

    if mode.contains(Mode::S_ISGID) && fsuid != 0 {
        let in_supplementary_group = pcb.supplementary_gids().contains(&inode_gid);
        if pcb.fsgid() != inode_gid && !in_supplementary_group {
            mode.remove(Mode::S_ISGID);
        }
    }

    inode.chmod(mode)?;
    Ok(0)
}

fn do_chown(dentry: &Arc<Dentry>, uid: Option<Uid>, gid: Option<Uid>) -> SyscallRet {
    let dentry = dentry.clone().get_mount_to();
    if dentry.is_superblock_readonly()? {
        return Err(Errno::EROFS);
    }

    let inode = dentry.get_inode();
    let (inode_uid, inode_gid) = inode.owner()?;
    let pcb = current::pcb();
    let fsuid = pcb.fsuid();

    if fsuid != 0 {
        if fsuid != inode_uid {
            return Err(Errno::EPERM);
        }

        if uid.is_some_and(|uid| uid != inode_uid) {
            return Err(Errno::EPERM);
        }

        if let Some(gid) = gid
            && gid != inode_gid
        {
            let in_supplementary_group = pcb.supplementary_gids().contains(&gid);
            if pcb.fsgid() != gid && !in_supplementary_group {
                return Err(Errno::EPERM);
            }
        }
    }

    inode.chown(uid, gid)?;
    Ok(0)
}

pub fn fchmodat(dirfd: usize, uptr_path: UString, mode: usize) -> SyscallRet {
    let path = uptr_path.should_not_null()?.read_path()?;
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

    do_chmod(&dentry, mode)
}

pub fn fchmod(fd: usize, mode: usize) -> SyscallRet {
    let file = current::fdtable().lock().get(fd)?;
    let dentry = file.get_dentry().ok_or(Errno::EINVAL)?;
    do_chmod(dentry, mode)
}

pub fn fchownat(dirfd: usize, uptr_path: UString, uid: usize, gid: usize, flags: usize) -> SyscallRet {
    let flags = AtFlags::from_bits(flags).ok_or(Errno::EINVAL)?;
    let path = if flags.contains(AtFlags::AT_EMPTY_PATH) {
        String::new()
    } else {
        uptr_path.should_not_null()?;
        uptr_path.read_path()?
    };

    if path.is_empty() && !flags.contains(AtFlags::AT_EMPTY_PATH) {
        return Err(Errno::ENOENT);
    }

    let dentry = if path.is_empty() {
        if dirfd as isize == AT_FDCWD {
            current::with_cwd(|cwd| Ok(cwd))?
        } else {
            current::fdtable()
                .lock()
                .get(dirfd)?
                .get_dentry()
                .cloned()
                .ok_or(Errno::EINVAL)?
        }
    } else {
        let helper = if flags.contains(AtFlags::AT_SYMLINK_NOFOLLOW) {
            vfs::load_dentry_at_nofollow
        } else {
            vfs::load_dentry_at
        };
        if path.starts_with('/') || dirfd as isize == AT_FDCWD {
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
    do_chown(&dentry, uid, gid)
}

pub fn fchown(fd: usize, uid: usize, gid: usize) -> SyscallRet {
    let file = current::fdtable().lock().get(fd)?;

    let uid = uid as Uid;
    let gid = gid as Uid;

    let dentry = file.get_dentry().ok_or(Errno::EINVAL)?;
    let uid = if uid == Uid::MAX { None } else { Some(uid as Uid) };
    let gid = if gid == Uid::MAX { None } else { Some(gid as Uid) };
    do_chown(dentry, uid, gid)
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
        let _ = current::pcb().send_signal(signum::SIGXFSZ, SiCode::SI_KERNEL, 0, KSiFields::Empty, None);
        return Err(Errno::EFBIG);
    }

    Ok(())
}

pub fn truncate64(uptr_path: UString, length: usize) -> SyscallRet {
    let path = uptr_path.should_not_null()?.read_path()?;
    if path.is_empty() {
        return Err(Errno::ENOENT);
    }

    let length = truncate_length(length)?;
    let dentry = current::with_cwd(|cwd| vfs::load_dentry_at(&cwd, &path))?;
    let inode = dentry.get_inode();
    let mode = inode.mode()?;
    if (mode & Mode::S_IFMT) == Mode::S_IFDIR {
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
    file.downcast_arc::<RandomAccessFile>()
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

    // file.downcast_arc::<RandomAccessFile>()
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

bitflags! {
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    struct MountFlags: usize {
        const RDONLY = 0x1;
        const REMOUNT = 0x20;
    }
}

bitflags! {
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    struct FlockOperation: usize {
        const SHARED = 1;
        const EXCLUSIVE = 1 << 1;
        const NONBLOCK = 1 << 2;
        const UNLOCK = 1 << 3;
    }
}

pub fn flock(fd: usize, operation: usize) -> SyscallRet {
    let operation = FlockOperation::from_bits(operation).ok_or(Errno::EINVAL)?;
    let file = current::fdtable().lock().get(fd)?;
    let inode = file.get_inode().ok_or(Errno::EINVAL)?;
    let lock_state = inode.lock_state().ok_or(Errno::EINVAL)?;

    let lock_operation = operation & (FlockOperation::SHARED | FlockOperation::EXCLUSIVE | FlockOperation::UNLOCK);
    let request_type = if lock_operation == FlockOperation::SHARED {
        Some(BsdFlockType::Shared)
    } else if lock_operation == FlockOperation::EXCLUSIVE {
        Some(BsdFlockType::Exclusive)
    } else if lock_operation == FlockOperation::UNLOCK {
        None
    } else {
        return Err(Errno::EINVAL);
    };

    let owner = file.flock_owner_id();
    if request_type.is_none() {
        let mut state = lock_state.lock();
        if state.bsd.remove_owner(owner) {
            state.bsd.wake_all();
        }
        return Ok(0);
    }

    let request_type = request_type.unwrap();
    let blocking = !operation.contains(FlockOperation::NONBLOCK);
    loop {
        let mut state = lock_state.lock();
        if state.bsd.get_conflict(owner, request_type).is_none() {
            state.bsd.apply(owner, Some(request_type));
            state.bsd.wake_all();
            return Ok(0);
        }

        if !blocking {
            return Err(Errno::EAGAIN);
        }

        state.bsd.wait_current();
        drop(state);

        current::schedule();
        match current::task().take_wakeup_event().unwrap() {
            Event::IOComplete => {}
            Event::Signal => return Err(Errno::EINTR),
            event => unreachable!("unexpected event while waiting on flock lock: {:?}", event),
        }
    }
}

pub fn mount(
    uptr_source: UString,
    uptr_target: UString,
    uptr_fstype: UString,
    flags: usize,
    _data: usize,
) -> SyscallRet {
    let flags = MountFlags::from_bits_truncate(flags);

    uptr_target.should_not_null()?;

    let target = uptr_target.read_path()?;
    let options = MountOptions::new(flags.contains(MountFlags::RDONLY));

    if flags.contains(MountFlags::REMOUNT) {
        current::with_cwd(|cwd| vfs::remount(&cwd, &target, options))?;
        return Ok(0);
    }

    uptr_fstype.should_not_null()?;
    let fstype = uptr_fstype.read_string()?;

    let device = if !uptr_source.is_null() {
        let source = uptr_source.read_path()?;
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

    current::with_cwd(|cwd| vfs::mount(&cwd, &target, &fstype, device, options))?;

    Ok(0)
}

pub fn umount2(uptr_target: UString, flags: usize) -> SyscallRet {
    uptr_target.should_not_null()?;

    if flags != 0 {
        return Err(Errno::EINVAL);
    }

    let target = uptr_target.read_path()?;
    current::with_cwd(|cwd| vfs::unmount(&cwd, &target))?;
    Ok(0)
}
