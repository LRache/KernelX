use alloc::sync::Arc;
use alloc::vec::Vec;
use bitflags::bitflags;
use num_enum::TryFromPrimitive;

use crate::fs::file::{FileFlags, FileOps, RandomAccessFile};
use crate::fs::{FileType, Perm, PermFlags, vfs};
use crate::kernel::errno::{Errno, SysResult};
use crate::kernel::event::Event;
use crate::kernel::ipc::{SiCode, SiSigChld, SigInfo, SignalNum, signum};
use crate::kernel::scheduler::current::{copy_from_user, copy_to_user};
use crate::kernel::scheduler::{Tid, current};
use crate::kernel::syscall::uptr::{UArray, UPtr, UString, UserPointer};
use crate::kernel::syscall::{SyscallRet, UserStruct};
use crate::kernel::task::def::TaskCloneFlags;
use crate::kernel::task::fdtable::FDFlags;
use crate::kernel::task::pidfd::PidFile;
use crate::kernel::task::{ExitStatus, PCB, manager};
use crate::kernel::uapi::{OpenFlags, Uid};
use crate::kernel::{config, scheduler, task};

use super::uid;

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

pub fn setpriority(which: usize, who: usize, prio: usize) -> SyscallRet {
    const PRIO_PROCESS: usize = 0;
    const PRIO_MIN: isize = -20;
    const PRIO_MAX: isize = 19;

    if which != PRIO_PROCESS {
        return Err(Errno::EINVAL);
    }

    let prio = (prio as isize).clamp(PRIO_MIN, PRIO_MAX);

    let caller = current::pcb().clone();
    let target = if who == 0 {
        caller.clone()
    } else {
        let pid = who as Tid;
        if pid < 0 {
            return Err(Errno::EINVAL);
        }
        find_process(pid).ok_or(Errno::ESRCH)?
    };

    if caller.euid() != 0 && prio < target.nice() {
        return Err(Errno::EPERM);
    }

    target.set_nice(prio);
    Ok(0)
}

bitflags! {
    struct PidFdFlags: usize {
        const NONBLOCK = OpenFlags::O_NONBLOCK.bits();
    }
}

fn find_process(pid: Tid) -> Option<Arc<task::PCB>> {
    manager::get(pid).map(|tcb| tcb.parent().clone())
}

fn can_access_pidfd_target(target: &task::PCB) -> bool {
    let caller = current::pcb();
    if caller.euid() == 0 {
        return true;
    }

    let caller_uids = [caller.uid(), caller.euid(), caller.suid()];
    let target_uids = [target.uid(), target.euid(), target.suid()];
    if !caller_uids.iter().all(|uid| target_uids.contains(uid)) {
        return false;
    }

    let caller_gids = [caller.gid(), caller.egid(), caller.sgid()];
    let target_gids = [target.gid(), target.egid(), target.sgid()];
    caller_gids.iter().all(|gid| target_gids.contains(gid))
}

fn install_pidfd(pcb: &Arc<PCB>, blocked: bool) -> SysResult<usize> {
    let pidfd = Arc::new(PidFile::new(
        pcb,
        FileFlags {
            readable: true,
            writable: false,
            blocked,
            append: false,
            direct: false,
        },
    ));
    current::fdtable().lock().push(pidfd, FDFlags { cloexec: true })
}

pub fn pidfd_open(pid: usize, flags: usize) -> SyscallRet {
    let pid = pid as Tid;
    if pid <= 0 {
        return Err(Errno::EINVAL);
    }

    let flags = PidFdFlags::from_bits(flags).ok_or(Errno::EINVAL)?;
    let pcb = find_process(pid).ok_or(Errno::ESRCH)?;
    install_pidfd(&pcb, !flags.contains(PidFdFlags::NONBLOCK))
}

pub fn pidfd_getfd(pidfd: usize, targetfd: usize, flags: usize) -> SyscallRet {
    if flags != 0 {
        return Err(Errno::EINVAL);
    }

    let pidfd = current::fdtable()
        .lock()
        .get(pidfd)?
        .downcast_arc::<PidFile>()
        .map_err(|_| Errno::EBADF)?;
    let target = pidfd.pcb().ok_or(Errno::ESRCH)?;

    if target.is_exited() {
        return Err(Errno::ESRCH);
    }
    if !can_access_pidfd_target(&target) {
        return Err(Errno::EPERM);
    }

    let file = target.leader().ok_or(Errno::ESRCH)?.fdtable().lock().get(targetfd)?;
    let new_fd = current::fdtable().lock().push(file, FDFlags { cloexec: true })?;
    Ok(new_fd)
}

fn kcmp_object<T: ?Sized>(obj1: &Arc<T>, obj2: &Arc<T>) -> usize {
    match Arc::as_ptr(obj1).addr().cmp(&Arc::as_ptr(obj2).addr()) {
        core::cmp::Ordering::Equal => 0,
        core::cmp::Ordering::Less => 1,
        core::cmp::Ordering::Greater => 2,
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, TryFromPrimitive)]
#[repr(usize)]
enum KcmpType {
    File = 0,
    VM = 1,
}

pub fn kcmp(pid1: usize, pid2: usize, compare_type: usize, idx1: usize, idx2: usize) -> SyscallRet {
    let compare_type = KcmpType::try_from(compare_type).map_err(|_| Errno::EINVAL)?;
    let pid1 = Tid::try_from(pid1).map_err(|_| Errno::ESRCH)?;
    let pid2 = Tid::try_from(pid2).map_err(|_| Errno::ESRCH)?;
    if pid1 <= 0 || pid2 <= 0 {
        return Err(Errno::ESRCH);
    }

    let pcb1 = find_process(pid1).ok_or(Errno::ESRCH)?;
    let pcb2 = find_process(pid2).ok_or(Errno::ESRCH)?;

    let r = match compare_type {
        KcmpType::File => {
            let obj1 = pcb1.leader().ok_or(Errno::ESRCH)?.fdtable().lock().get(idx1)?;
            let obj2 = pcb2.leader().ok_or(Errno::ESRCH)?.fdtable().lock().get(idx2)?;
            kcmp_object(&obj1, &obj2)
        }
        KcmpType::VM => kcmp_object(
            pcb1.leader().ok_or(Errno::ESRCH)?.get_addrspace(),
            pcb2.leader().ok_or(Errno::ESRCH)?.get_addrspace(),
        ),
    };

    Ok(r)
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
    pidfd_addr: Option<UPtr<Tid>>,
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
    if flags.contains(CloneFlags::NEWUTS) && flags.contains(CloneFlags::THREAD) {
        return Err(Errno::EINVAL);
    }
    if flags.contains(CloneFlags::NEWUTS) && current::pcb().euid() != 0 {
        return Err(Errno::EPERM);
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

    if let Some(pidfd_addr) = args.pidfd_addr {
        let pidfd = install_pidfd(child.parent(), true)?;
        if let Err(err) = pidfd_addr.write(pidfd as Tid) {
            let _ = current::fdtable().lock().take(pidfd);
            return Err(err);
        }
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
            new_uts: flags.contains(CloneFlags::NEWUTS),
        },
        stack,
        tls: if flags.contains(CloneFlags::SETTLS) {
            Some(tls)
        } else {
            None
        },
        parent_tid_addr: uptr_parent_tid.into(),
        child_tid_addr: uptr_child_tid,
        pidfd_addr: None,
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
    let pidfd_addr = if flags.contains(CloneFlags::PIDFD) {
        let pidfd_uptr: UPtr<Tid> = (kargs.pidfd as usize).into();
        pidfd_uptr.read()?;
        Some(pidfd_uptr)
    } else {
        None
    };

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
            new_uts: flags.contains(CloneFlags::NEWUTS),
        },
        stack,
        tls: if flags.contains(CloneFlags::SETTLS) {
            Some(kargs.tls as usize)
        } else {
            None
        },
        parent_tid_addr: (kargs.parent_tid as usize).into(),
        child_tid_addr: kargs.child_tid as usize,
        pidfd_addr,
        exit_signal,
        flags,
    })
}

fn read_ustring_array(uarray: UArray<UString>) -> SysResult<Vec<fixedstr::tstr<255>>> {
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
        vec.push(p.read_string()?);
        i += 1;
    }
    Ok(vec)
}

fn do_execve(
    file: Arc<RandomAccessFile>,
    invoked_path: &str,
    uptr_argv: UArray<UString>,
    uptr_envp: UArray<UString>,
) -> SyscallRet {
    let argv = read_ustring_array(uptr_argv)?;
    let envp = read_ustring_array(uptr_envp)?;
    let mut argv_ref: Vec<&str> = argv.iter().map(|s| s.as_str()).collect();
    let envp_ref: Vec<&str> = envp.iter().map(|s| s.as_str()).collect();

    // See https://git.kernel.org/pub/scm/linux/kernel/git/torvalds/linux.git/commit/?id=dcd46d897adb
    if argv_ref.len() == 0 {
        argv_ref.push("");
    }

    current::pcb().exec(current::tcb(), file, invoked_path, &argv_ref, &envp_ref)?;
    current::tcb().wake_parent_waiting_vfork();
    Ok(0)
}

pub fn execve(uptr_path: UString, uptr_argv: UArray<UString>, uptr_envp: UArray<UString>) -> SyscallRet {
    uptr_path.should_not_null()?;

    let path = uptr_path.read_string()?;

    let file = current::with_root_cwd(|root, cwd| {
        vfs::openat_file(&root, &cwd, &path, FileFlags::readonly(), &Perm::current(PermFlags::X))
    })?
    .downcast_arc::<RandomAccessFile>()
    .map_err(|_| Errno::ENOEXEC)?;

    do_execve(file, &path, uptr_argv, uptr_envp)
}

bitflags! {
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    struct ExecveAtFlags: usize {
        const AT_SYMLINK_NOFOLLOW = 0x100;
        const AT_EMPTY_PATH = 0x1000;
    }
}

fn open_execveat_file(
    root: &Arc<crate::fs::Dentry>,
    dir: &Arc<crate::fs::Dentry>,
    path: &str,
    flags: ExecveAtFlags,
    perm: &Perm,
) -> SysResult<Arc<RandomAccessFile>> {
    let file = if flags.contains(ExecveAtFlags::AT_SYMLINK_NOFOLLOW) {
        let dentry = vfs::load_dentry_at_nofollow_with_perm(root, dir, path, perm)?;
        let inode = dentry.get_inode();
        if inode.inode_type()? == FileType::Symlink {
            return Err(Errno::ELOOP);
        }

        let mode = inode.mode()?;
        let (uid, gid) = inode.owner()?;
        if !mode.check_perm(perm, uid, gid) {
            return Err(Errno::EACCES);
        }

        inode.wrap_file(Some(dentry), FileFlags::readonly())
    } else {
        vfs::openat_file(root, dir, path, FileFlags::readonly(), perm)?
    };

    file.downcast_arc::<RandomAccessFile>().map_err(|_| Errno::ENOEXEC)
}

pub fn execveat(
    dirfd: usize,
    uptr_path: UString,
    uptr_argv: UArray<UString>,
    uptr_envp: UArray<UString>,
    flags: usize,
) -> SyscallRet {
    use super::def::AT_FDCWD;

    let flags = ExecveAtFlags::from_bits(flags).ok_or(Errno::EINVAL)?;
    let path = uptr_path.should_not_null()?.read_path()?;
    let perm = Perm::current(PermFlags::X);

    let (file, invoked_path) = if flags.contains(ExecveAtFlags::AT_EMPTY_PATH) && path.is_empty() {
        current::fdtable()
            .lock()
            .get(dirfd)?
            .downcast_arc::<RandomAccessFile>()
            .map_err(|_| Errno::ENOEXEC)
            .map(|file| {
                let invoked_path = file.get_dentry().map(|dentry| dentry.get_path()).unwrap_or_default();
                (file, invoked_path)
            })?
    } else {
        if path.is_empty() {
            return Err(Errno::ENOENT);
        }

        // When pathname is absolute, dirfd can be ignored.
        let file = if path.starts_with('/') {
            current::with_root_cwd(|root, cwd| open_execveat_file(&root, &cwd, &path, flags, &perm))?
        } else if dirfd as isize == AT_FDCWD {
            current::with_root_cwd(|root, cwd| open_execveat_file(&root, &cwd, &path, flags, &perm))?
        } else {
            let dir_file = current::fdtable().lock().get(dirfd)?;
            let dir = dir_file.get_dentry().ok_or(Errno::ENOTDIR)?;
            current::with_root(|root| open_execveat_file(&root, dir, &path, flags, &perm))?
        };
        (file, path.as_str().into())
    };

    do_execve(file, &invoked_path, uptr_argv, uptr_envp)
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
        if let Some((child, status)) = pcb.wait_any_child(!options.contains(WaitOptions::WNOHANG))? {
            wait_pid = child.pid();
            exit_status = status;
        } else {
            return Ok(0);
        }
    } else if pid == 0 {
        // Wait for any child whose pgid equals the caller's pgid
        let caller_pgid = pcb.pgid();
        if let Some((child, status)) = pcb.wait_child_by_pgid(caller_pgid, !options.contains(WaitOptions::WNOHANG))? {
            wait_pid = child.pid();
            exit_status = status;
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

        if let Some((child, status)) = pcb.wait_child_by_pgid(target_pgid, !options.contains(WaitOptions::WNOHANG))? {
            wait_pid = child.pid();
            exit_status = status;
        } else {
            return Ok(0);
        }
    } else {
        // pid > 0: wait for specific child
        if let Some((child, status)) = pcb.wait_child(pid as i32, !options.contains(WaitOptions::WNOHANG))? {
            wait_pid = child.pid();
            exit_status = status;
        } else {
            return Ok(0);
        }
    }

    if !status.is_null() {
        status.write(exit_status.as_wstatus())?;
    }

    Ok(wait_pid as usize)
}

#[repr(usize)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, TryFromPrimitive)]
enum WaitIdType {
    All = 0,
    Pid = 1,
    Pgid = 2,
    PidFd = 3,
}

bitflags! {
    struct WaitIdOptions: usize {
        const WNOHANG    = 1 << 0;
        const WEXITED    = 1 << 2;
        const WSTOPPED   = 1 << 1;
        const WCONTINUED = 1 << 3;
        const WNOWAIT    = 1 << 24;
    }
}

fn waitid_siginfo(child: &PCB, status: ExitStatus) -> SigInfo {
    let si_code = match status {
        ExitStatus::Normal(_) => SiCode::CLD_EXITED,
        ExitStatus::Signal { coredump: true, .. } => SiCode::CLD_DUMPED,
        ExitStatus::Signal { coredump: false, .. } => SiCode::CLD_KILLED,
    };

    SigInfo::sigchld(
        signum::SIGCHLD.num() as i32,
        si_code,
        SiSigChld {
            si_pid: child.pid(),
            si_uid: child.uid(),
            si_status: status.si_status(),
            si_utime: 0,
            si_stime: 0,
        },
    )
}

pub fn waitid(
    idtype: usize,
    id: usize,
    uptr_siginfo: UPtr<SigInfo>,
    options: usize,
    _user_rusage: usize,
) -> SyscallRet {
    let idtype = WaitIdType::try_from(idtype).map_err(|_| Errno::EINVAL)?;
    let options = WaitIdOptions::from_bits(options).ok_or(Errno::EINVAL)?;

    if !options.contains(WaitIdOptions::WEXITED)
        || options.intersects(WaitIdOptions::WSTOPPED | WaitIdOptions::WCONTINUED | WaitIdOptions::WNOWAIT)
    {
        return Err(Errno::EINVAL);
    }

    match idtype {
        WaitIdType::All => {
            let (child, status) = match current::pcb().wait_any_child(!options.contains(WaitIdOptions::WNOHANG))? {
                Some(result) => result,
                None => {
                    if !uptr_siginfo.is_null() {
                        uptr_siginfo.write(SigInfo::empty())?;
                    }
                    return Ok(0);
                }
            };

            if !uptr_siginfo.is_null() {
                uptr_siginfo.write(waitid_siginfo(&child, status))?;
            }

            Ok(0)
        }
        WaitIdType::Pid => {
            let target_pid = i32::try_from(id).map_err(|_| Errno::EINVAL)?;
            let (child, status) =
                match current::pcb().wait_child(target_pid, !options.contains(WaitIdOptions::WNOHANG))? {
                    Some(result) => result,
                    None => {
                        if !uptr_siginfo.is_null() {
                            uptr_siginfo.write(SigInfo::empty())?;
                        }
                        return Ok(0);
                    }
                };

            if !uptr_siginfo.is_null() {
                uptr_siginfo.write(waitid_siginfo(&child, status))?;
            }

            Ok(0)
        }
        WaitIdType::PidFd => {
            let pidfd = current::fdtable()
                .lock()
                .get(id)?
                .downcast_arc::<PidFile>()
                .map_err(|_| Errno::EINVAL)?;
            let child = pidfd.pcb().ok_or(Errno::ECHILD)?;
            let child_pid = child.pid() as usize;
            let block = pidfd.block() && !options.contains(WaitIdOptions::WNOHANG);

            let status = match current::pcb().wait_child(child_pid as i32, block)? {
                Some(result) => result,
                None => {
                    if !uptr_siginfo.is_null() {
                        uptr_siginfo.write(SigInfo::empty())?;
                    }
                    if !pidfd.block() && !options.contains(WaitIdOptions::WNOHANG) {
                        return Err(Errno::EAGAIN);
                    }
                    return Ok(0);
                }
            };
            let (child, status) = status;

            if !uptr_siginfo.is_null() {
                uptr_siginfo.write(waitid_siginfo(&child, status))?;
            }

            Ok(0)
        }
        WaitIdType::Pgid => {
            let target_pgid = if id == 0 {
                current::pcb().pgid()
            } else {
                i32::try_from(id).map_err(|_| Errno::EINVAL)?
            };
            let (child, status) =
                match current::pcb().wait_child_by_pgid(target_pgid, !options.contains(WaitIdOptions::WNOHANG))? {
                    Some(result) => result,
                    None => {
                        if !uptr_siginfo.is_null() {
                            uptr_siginfo.write(SigInfo::empty())?;
                        }
                        return Ok(0);
                    }
                };

            if !uptr_siginfo.is_null() {
                uptr_siginfo.write(waitid_siginfo(&child, status))?;
            }

            Ok(0)
        }
    }
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
    let path = uptr_path.should_not_null()?.read_path()?;
    if path.len() >= config::MAX_FILENAME_LEN {
        return Err(Errno::ENAMETOOLONG);
    }
    let dentry = current::with_root_cwd(|root, cwd| vfs::load_dentry_at(&root, &cwd, &path))?;
    current::pcb().set_cwd(&dentry);
    Ok(0)
}

pub fn fchdir(fd: usize) -> SysResult<usize> {
    let file = current::fdtable().lock().get(fd)?;
    let dentry = file.get_dentry().ok_or(Errno::ENOTDIR)?;
    current::pcb().set_cwd(&dentry);
    Ok(0)
}

pub fn chroot(uptr_path: UString) -> SysResult<usize> {
    let path = uptr_path.should_not_null()?.read_path()?;
    if path.len() >= config::MAX_FILENAME_LEN {
        return Err(Errno::ENAMETOOLONG);
    }

    let perm = Perm::current(PermFlags::X);
    let dentry = current::with_root_cwd(|root, cwd| vfs::load_dentry_at_with_perm(&root, &cwd, &path, &perm))?;
    dentry.check_search_perm(&perm)?;

    if current::euid() != 0 {
        return Err(Errno::EPERM);
    }

    current::pcb().set_root(&dentry);
    Ok(0)
}

pub fn setfsuid(fsuid: usize) -> SyscallRet {
    let pcb = current::pcb();
    let old_fsuid = pcb.fsuid();
    let fsuid = fsuid as Uid;

    if fsuid != Uid::MAX
        && (pcb.euid() == 0 || fsuid == pcb.uid() || fsuid == pcb.euid() || fsuid == pcb.suid() || fsuid == old_fsuid)
    {
        pcb.set_fsuid(fsuid);
    }

    Ok(old_fsuid as usize)
}

pub fn setfsgid(fsgid: usize) -> SyscallRet {
    let pcb = current::pcb();
    let old_fsgid = pcb.fsgid();
    let fsgid = fsgid as Uid;

    if fsgid != Uid::MAX
        && (pcb.euid() == 0 || fsgid == pcb.gid() || fsgid == pcb.egid() || fsgid == pcb.sgid() || fsgid == old_fsgid)
    {
        pcb.set_fsgid(fsgid);
    }

    Ok(old_fsgid as usize)
}

pub fn prctl(option: usize, arg2: usize, arg3: usize, arg4: usize, arg5: usize) -> SyscallRet {
    const PR_CAPBSET_DROP: usize = 24;

    let _ = (arg3, arg4, arg5);

    match option {
        PR_CAPBSET_DROP => uid::drop_capability_from_bounding(arg2),
        _ => Ok(0),
    }
}
