use alloc::string::String;
use alloc::sync::Arc;
use alloc::vec::Vec;
use bitflags::bitflags;

use crate::fs::file::{File, FileFlags};
use crate::fs::{Perm, PermFlags, vfs};
use crate::kernel::errno::{Errno, SysResult};
use crate::kernel::event::Event;
use crate::kernel::ipc::SignalNum;
use crate::kernel::scheduler::current::{copy_from_user, copy_to_user};
use crate::kernel::scheduler::{Tid, current};
use crate::kernel::syscall::uptr::{UArray, UPtr, UString, UserPointer};
use crate::kernel::syscall::{SyscallRet, UserStruct};
use crate::kernel::task::ExitStatus;
use crate::kernel::task::def::TaskCloneFlags;
use crate::kernel::uapi::Uid;
use crate::kernel::{config, scheduler, task};

pub fn sched_yield() -> SyscallRet {
    current::schedule();
    Ok(0)
}

pub fn getpid() -> SyscallRet {
    let pcb = current::pcb();
    Ok(pcb.pid() as usize)
}

pub fn gettid() -> SyscallRet {
    let tcb = current::tcb();
    Ok(tcb.tid() as usize)
}

pub fn getppid() -> SyscallRet {
    let pcb = current::pcb();
    let ppid = pcb.parent.lock().as_ref().map_or(0, |p| p.pid());
    Ok(ppid as usize)
}

pub fn getpgid(pid: usize) -> SyscallRet {
    let pid = pid as i32;
    if pid == 0 {
        Ok(current::pcb().pgid() as usize)
    } else {
        let tcb = crate::kernel::task::manager::get(pid).ok_or(Errno::ESRCH)?;
        Ok(tcb.parent().pgid() as usize)
    }
}

pub fn setpgid(pid: usize, pgid: usize) -> SyscallRet {
    let pid = pid as Tid;
    let pgid = pgid as Tid;
    if pid < 0 || pgid < 0 {
        return Err(Errno::EINVAL);
    }

    let current_pcb = current::pcb().clone();
    let current_pid = current_pcb.pid();
    let target_pid = if pid == 0 { current_pid } else { pid };

    let target_pcb = if target_pid == current_pid {
        current_pcb.clone()
    } else {
        let target_tcb = task::manager::get(target_pid).ok_or(Errno::ESRCH)?;
        let target_pcb = target_tcb.parent().clone();
        let is_child = target_pcb
            .parent
            .lock()
            .as_ref()
            .is_some_and(|parent| Arc::ptr_eq(parent, &current_pcb));
        if !is_child {
            return Err(Errno::ESRCH);
        }
        target_pcb
    };

    if target_pid != current_pid && target_pcb.has_execed() {
        return Err(Errno::EACCES);
    }

    if target_pcb.is_session_leader() {
        return Err(Errno::EPERM);
    }

    if target_pcb.sid() != current_pcb.sid() {
        return Err(Errno::EPERM);
    }

    let target_pgid = if pgid == 0 { target_pid } else { pgid };

    // Joining an existing process group requires the group leader to exist
    // and be in the same session.
    if target_pgid != target_pid && task::manager::get(target_pgid).is_none() {
        return Err(Errno::EPERM);
    }
    if target_pgid != target_pid {
        let group_tcb = task::manager::get(target_pgid).ok_or(Errno::EPERM)?;
        let group_pcb = group_tcb.parent().clone();
        if group_pcb.sid() != current_pcb.sid() {
            return Err(Errno::EPERM);
        }
    }

    target_pcb.set_pgid(target_pgid);

    Ok(0)
}

pub fn setsid() -> SyscallRet {
    let pcb = current::pcb();
    if pcb.pgid() == pcb.pid() {
        return Err(Errno::EPERM);
    }
    pcb.set_sid(pcb.pid());
    pcb.set_pgid(pcb.pid());
    Ok(pcb.pid() as usize)
}

bitflags! {
    #[derive(Debug)]
    struct CloneFlags: i32 {
        const VM = 0x0000100;
        const FS = 0x0000200;
        const FILES = 0x0000400;
        const SIGHAND = 0x00000800;
        const PIDFD = 0x00001000;
        const VFORK = 0x0000_4000;
        const PARENT = 0x00008000;
        const THREAD = 0x00010000;
        const SYSVSEM = 0x00040000;
        const SETTLS = 0x00080000;
        const PARENT_SETTID = 0x00100000;
        const CHILD_CLEARTID = 0x00200000;
        const CLONE_DETACHED = 0x00400000;
        const CHILD_SETTID = 0x01000000;
        const UNTRACED = 0x00800000;
        const NEWCGROUP = 0x02000000;
        const NEWUTS = 0x04000000;
        const NEWIPC = 0x08000000;
        const NEWUSER = 0x10000000;
        const NEWPID = 0x20000000;
        const NEWNET = 0x40000000;
    }
}

#[derive(Debug)]
struct CloneArgs {
    flags: CloneFlags,
    task_flags: TaskCloneFlags,
    stack: usize,
    tls: Option<usize>,
    parent_tid_addr: UPtr<Tid>,
    child_tid_addr: usize,
    exit_signal: SignalNum,
}

/// Validate clone flag combinations (matching Linux semantics).
fn check_clone_flags(flags: &CloneFlags) -> SysResult<()> {
    // CLONE_THREAD requires CLONE_SIGHAND
    if flags.contains(CloneFlags::THREAD) && !flags.contains(CloneFlags::SIGHAND) {
        return Err(Errno::EINVAL);
    }
    // CLONE_SIGHAND requires CLONE_VM
    if flags.contains(CloneFlags::SIGHAND) && !flags.contains(CloneFlags::VM) {
        return Err(Errno::EINVAL);
    }
    Ok(())
}

fn do_clone(args: CloneArgs) -> SyscallRet {
    let child = current::pcb().clone_task(current::tcb(), args.stack, &args.task_flags, args.tls, args.exit_signal)?;
    let child_tid = child.tid();

    if args.flags.contains(CloneFlags::CHILD_SETTID) {
        let _ = child.get_addrspace().copy_to_user(args.child_tid_addr, child_tid);
    }

    if args.flags.contains(CloneFlags::CHILD_CLEARTID) {
        child.set_tid_address(args.child_tid_addr);
    }

    if args.flags.contains(CloneFlags::PARENT_SETTID) {
        args.parent_tid_addr.write(child_tid)?;
    }

    if args.flags.contains(CloneFlags::VFORK) {
        child.set_parent_waiting_vfork(Some(current::task().clone()));
        scheduler::push_task(child);

        let event = current::block_uninterruptible("vfork");

        match event {
            Event::VFork => {}
            _ => unreachable!(),
        }
    } else {
        scheduler::push_task(child);
    }

    Ok(child_tid as usize)
}

pub fn clone(flags: usize, stack: usize, uptr_parent_tid: UPtr<Tid>, tls: usize, uptr_child_tid: usize) -> SyscallRet {
    let exit_signal = SignalNum::try_from((flags & 0xff) as u32)?;
    let flags = CloneFlags::from_bits((flags & !0xff) as i32).ok_or(Errno::EINVAL)?;

    check_clone_flags(&flags)?;

    do_clone(CloneArgs {
        task_flags: TaskCloneFlags {
            vm: flags.contains(CloneFlags::VM),
            files: flags.contains(CloneFlags::FILES),
            thread: flags.contains(CloneFlags::THREAD),
            parent: flags.contains(CloneFlags::PARENT),
            vfork: flags.contains(CloneFlags::VFORK),
        },
        stack,
        tls: if flags.contains(CloneFlags::SETTLS) {
            Some(tls)
        } else {
            None
        },
        parent_tid_addr: uptr_parent_tid.into(),
        child_tid_addr: uptr_child_tid,
        flags,
        exit_signal,
    })
}

/// clone3 `clone_args` struct layout from Linux UAPI
#[repr(C)]
#[derive(Clone, Copy)]
pub struct KernelCloneArgs {
    flags: u64,
    pidfd: u64,
    child_tid: u64,
    parent_tid: u64,
    exit_signal: u64,
    stack: u64,
    stack_size: u64,
    tls: u64,
}

impl UserStruct for KernelCloneArgs {}

const CLONE_ARGS_MIN_SIZE: usize = core::mem::size_of::<KernelCloneArgs>();

pub fn clone3(uargs: UPtr<KernelCloneArgs>, size: usize) -> SyscallRet {
    if size < CLONE_ARGS_MIN_SIZE {
        return Err(Errno::EINVAL);
    }

    let kargs = uargs.read()?;

    // If size is larger than the known struct, verify that the extra bytes are all zero.
    // This ensures forward compatibility: unknown fields must be zero.
    if size > CLONE_ARGS_MIN_SIZE {
        let extra_start = uargs.uaddr() + CLONE_ARGS_MIN_SIZE;
        let extra_len = size - CLONE_ARGS_MIN_SIZE;
        let mut extra = alloc::vec![0u8; extra_len];
        copy_from_user::slice(extra_start, &mut extra)?;
        if extra.iter().any(|&b| b != 0) {
            return Err(Errno::E2BIG);
        }
    }

    let exit_signal = SignalNum::try_from(kargs.exit_signal as u32)?;
    let flags = CloneFlags::from_bits((kargs.flags & !0xff) as i32).ok_or(Errno::EINVAL)?;

    check_clone_flags(&flags)?;

    // Validate pidfd address if CLONE_PIDFD is set
    if flags.contains(CloneFlags::PIDFD) {
        let pidfd_uptr: UPtr<i32> = (kargs.pidfd as usize).into();
        pidfd_uptr.read()?;
    }

    // stack and stack_size must both be zero or both be non-zero
    if (kargs.stack == 0) != (kargs.stack_size == 0) {
        return Err(Errno::EINVAL);
    }

    let stack = if kargs.stack != 0 {
        (kargs.stack + kargs.stack_size) as usize
    } else {
        0
    };

    do_clone(CloneArgs {
        task_flags: TaskCloneFlags {
            vm: flags.contains(CloneFlags::VM),
            files: flags.contains(CloneFlags::FILES),
            thread: flags.contains(CloneFlags::THREAD),
            parent: flags.contains(CloneFlags::PARENT),
            vfork: flags.contains(CloneFlags::VFORK),
        },
        stack,
        tls: if flags.contains(CloneFlags::SETTLS) {
            Some(kargs.tls as usize)
        } else {
            None
        },
        parent_tid_addr: (kargs.parent_tid as usize).into(),
        child_tid_addr: kargs.child_tid as usize,
        exit_signal,
        flags,
    })
}

fn read_ustring_array(uarray: UArray<UString>) -> SysResult<Vec<String>> {
    if uarray.is_null() {
        return Ok(Vec::new());
    }

    let mut vec = Vec::new();
    let mut i = 0;
    loop {
        let p = uarray.index(i).read()?;
        if p.is_null() {
            break;
        }
        vec.push(p.read()?);
        i += 1;
    }
    Ok(vec)
}

fn do_execve(file: Arc<File>, uptr_argv: UArray<UString>, uptr_envp: UArray<UString>) -> SyscallRet {
    let argv = read_ustring_array(uptr_argv)?;
    let envp = read_ustring_array(uptr_envp)?;
    let mut argv_ref: Vec<&str> = argv.iter().map(|s| s.as_str()).collect();
    let envp_ref: Vec<&str> = envp.iter().map(|s| s.as_str()).collect();

    // See https://git.kernel.org/pub/scm/linux/kernel/git/torvalds/linux.git/commit/?id=dcd46d897adb
    if argv_ref.len() == 0 {
        argv_ref.push("");
    }

    current::pcb().exec(current::tcb(), file, &argv_ref, &envp_ref)?;
    current::tcb().wake_parent_waiting_vfork();
    Ok(0)
}

pub fn execve(uptr_path: UString, uptr_argv: UArray<UString>, uptr_envp: UArray<UString>) -> SyscallRet {
    uptr_path.should_not_null()?;

    let path = uptr_path.read_fixed()?;

    let file =
        current::with_cwd(|cwd| vfs::openat_file(&cwd, &path, FileFlags::dontcare(), &Perm::current(PermFlags::X)))?
            .downcast_arc::<File>()
            .map_err(|_| Errno::ENOEXEC)?;

    do_execve(file, uptr_argv, uptr_envp)
}

pub fn execveat(
    dirfd: usize,
    uptr_path: UString,
    uptr_argv: UArray<UString>,
    uptr_envp: UArray<UString>,
    flags: usize,
) -> SyscallRet {
    use super::def::AT_FDCWD;

    const AT_EMPTY_PATH: usize = 0x1000;

    let path = if uptr_path.is_null() {
        None
    } else {
        Some(uptr_path.read_fixed()?)
    };
    let path = path.as_deref().unwrap_or("");

    let file = if flags & AT_EMPTY_PATH != 0 && path.is_empty() {
        current::fdtable()
            .lock()
            .get(dirfd)?
            .downcast_arc::<File>()
            .map_err(|_| Errno::ENOEXEC)?
    } else {
        if path.is_empty() {
            return Err(Errno::ENOENT);
        }
        // When pathname is absolute, dirfd can be ignored.
        if path.starts_with('/') {
            current::with_cwd(|cwd| vfs::openat_file(&cwd, &path, FileFlags::dontcare(), &Perm::current(PermFlags::X)))?
        } else if dirfd as isize == AT_FDCWD {
            current::with_cwd(|cwd| vfs::openat_file(&cwd, &path, FileFlags::dontcare(), &Perm::current(PermFlags::X)))?
        } else {
            let dir_file = current::fdtable().lock().get(dirfd)?;
            let dir = dir_file.get_dentry().ok_or(Errno::ENOTDIR)?;
            vfs::openat_file(dir, &path, FileFlags::dontcare(), &Perm::current(PermFlags::X))?
        }
        .downcast_arc::<File>()
        .map_err(|_| Errno::ENOEXEC)?
    };

    do_execve(file, uptr_argv, uptr_envp)
}

bitflags! {
    pub struct WaitOptions: usize {
        const WNOHANG   = 1 << 0;
        const WUNTRACED = 1 << 1;
        const __WALL    = 1 << 30; // TODO: Implement __WALL
    }
}

pub fn wait4(pid: usize, status: UPtr<u32>, options: usize, _user_rusages: usize) -> Result<usize, Errno> {
    let pcb = current::pcb();
    let options = WaitOptions::from_bits(options).ok_or(Errno::EINVAL)?;
    let pid = pid as i32;

    let wait_pid;
    let exit_status: ExitStatus;

    if pid == -1 {
        // Wait for any child
        if let Some(result) = pcb.wait_any_child(!options.contains(WaitOptions::WNOHANG))? {
            wait_pid = result.0;
            exit_status = result.1;
        } else {
            return Ok(0);
        }
    } else if pid == 0 {
        // Wait for any child whose pgid equals the caller's pgid
        let caller_pgid = pcb.pgid();
        if let Some(result) = pcb.wait_child_by_pgid(caller_pgid, !options.contains(WaitOptions::WNOHANG))? {
            wait_pid = result.0;
            exit_status = result.1;
        } else {
            return Ok(0);
        }
    } else if pid < -1 {
        // Why
        if pid == i32::MIN {
            return Err(Errno::ESRCH);
        }

        // Wait for any child whose pgid equals -pid
        let target_pgid = -pid;

        if let Some(result) = pcb.wait_child_by_pgid(target_pgid, !options.contains(WaitOptions::WNOHANG))? {
            wait_pid = result.0;
            exit_status = result.1;
        } else {
            return Ok(0);
        }
    } else {
        // pid > 0: wait for specific child
        if let Some(result) = pcb.wait_child(pid as i32, !options.contains(WaitOptions::WNOHANG))? {
            wait_pid = pid as i32;
            exit_status = result;
        } else {
            return Ok(0);
        }
    }

    if !status.is_null() {
        status.write(exit_status.as_wstatus())?;
    }

    Ok(wait_pid as usize)
}

pub fn exit(code: usize) -> SyscallRet {
    let tcb = current::tcb();
    tcb.exit(ExitStatus::Normal(code as u8));

    tcb.wake_parent_waiting_vfork();

    Ok(0)
}

pub fn exit_group(code: usize) -> SyscallRet {
    let pcb = current::pcb();
    pcb.exit(ExitStatus::Normal(code as u8));

    current::tcb().wake_parent_waiting_vfork();

    Ok(0)
}

pub fn set_tid_address(tid_address: usize) -> SyscallRet {
    let tcb = current::tcb();
    tcb.set_tid_address(tid_address);
    Ok(tcb.tid() as usize)
}

pub fn getcwd(ubuf: usize, size: usize) -> SysResult<usize> {
    let cwd = current::with_cwd(|dentry| dentry.get_path());
    copy_to_user::string(ubuf, &cwd, size)
}

pub fn chdir(uptr_path: UString) -> SysResult<usize> {
    let path = uptr_path.should_not_null()?.read_fixed()?;
    if path.len() >= config::MAX_FILENAME_LEN {
        return Err(Errno::ENAMETOOLONG);
    }
    let dentry = current::with_cwd(|cwd| vfs::load_dentry_at(&cwd, &path))?;
    current::pcb().set_cwd(&dentry);
    Ok(0)
}

pub fn fchdir(fd: usize) -> SysResult<usize> {
    let file = current::fdtable().lock().get(fd)?;
    let dentry = file.get_dentry().ok_or(Errno::ENOTDIR)?;
    current::pcb().set_cwd(&dentry);
    Ok(0)
}

pub fn setfsuid(fsuid: usize) -> SyscallRet {
    let pcb = current::pcb();
    let old_fsuid = pcb.fsuid();
    let fsuid = fsuid as Uid;

    if pcb.euid() == 0 || fsuid == pcb.uid() || fsuid == pcb.euid() || fsuid == pcb.suid() || fsuid == old_fsuid {
        pcb.set_fsuid(fsuid);
    }

    Ok(old_fsuid as usize)
}

pub fn setfsgid(fsgid: usize) -> SyscallRet {
    let pcb = current::pcb();
    let old_fsgid = pcb.fsgid();
    let fsgid = fsgid as Uid;

    if pcb.euid() == 0 || fsgid == pcb.gid() || fsgid == pcb.egid() || fsgid == pcb.sgid() || fsgid == old_fsgid {
        pcb.set_fsgid(fsgid);
    }

    Ok(old_fsgid as usize)
}
