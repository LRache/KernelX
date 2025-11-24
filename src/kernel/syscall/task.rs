use alloc::string::String;
use alloc::vec::Vec;
use bitflags::bitflags;

use crate::fs::file::FileFlags;
use crate::fs::{Perm, PermFlags, vfs};
use crate::kernel::errno::{Errno, SysResult};
use crate::kernel::event::Event;
use crate::kernel::scheduler;
use crate::kernel::scheduler::current::{copy_from_user, copy_to_user};
use crate::kernel::scheduler::{Task, current};
use crate::kernel::syscall::SyscallRet;
use crate::kernel::syscall::uptr::{UArray, UPtr, UString, UserPointer};
use crate::kernel::task::Tid;
use crate::kernel::task::def::TaskCloneFlags;

pub fn sched_yield() -> SyscallRet {
    current::schedule();
    Ok(0)
}

pub fn getpid() -> SyscallRet {
    let pcb = current::pcb();
    Ok(pcb.get_pid() as usize)
}

pub fn gettid() -> SyscallRet {
    let tcb = current::tcb();
    Ok(tcb.get_tid() as usize)
}

pub fn getppid() -> SyscallRet {
    let pcb = current::pcb();
    let ppid = pcb.parent.lock().as_ref().map_or(0, |p| p.get_pid());
    Ok(ppid as usize)
}

pub fn setsid() -> SyscallRet {
    let pcb = current::pcb();
    // pcb.set_sid();
    Ok(pcb.get_pid() as usize)
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

pub fn clone(
    flags: usize,
    stack: usize,
    uptr_parent_tid: UPtr<Tid>,
    tls: usize,
    uptr_child_tid: usize,
) -> SyscallRet {
    let flags = CloneFlags::from_bits((flags & !0xff) as i32).ok_or(Errno::EINVAL)?;

    // kinfo!("clone: flags={:?}", flags);

    let task_flags = TaskCloneFlags {
        vm: flags.contains(CloneFlags::VM),
        files: flags.contains(CloneFlags::FILES),
        thread: flags.contains(CloneFlags::THREAD),
    };

    let tls = if flags.contains(CloneFlags::SETTLS) {
        Some(tls)
    } else {
        None
    };

    let child = current::pcb().clone_task(current::tcb(), stack, &task_flags, tls)?;

    if flags.contains(CloneFlags::CHILD_SETTID) {
        let _ = child.get_addrspace().copy_to_user(uptr_child_tid, 0 as Tid);
    }

    if flags.contains(CloneFlags::CHILD_CLEARTID) {
        child.set_tid_address(uptr_child_tid);
    }

    let child_tid = child.get_tid();

    if flags.contains(CloneFlags::PARENT_SETTID) {
        uptr_parent_tid.write(child_tid)?;
    }

    if flags.contains(CloneFlags::VFORK) {
        // timer::add_timer(current::tcb().clone(), Duration::from_secs(1));
        // current::tcb().block_uninterruptible("vfork");
        scheduler::block_task_uninterruptible(current::task(), "vfork");

        child.set_parent_waiting_vfork(Some(current::task().clone()));
        scheduler::push_task(child);

        current::schedule();

        match current::tcb().take_wakeup_event().unwrap() {
            Event::VFork => {}
            _ => unreachable!(),
        }
    } else {
        scheduler::push_task(child);
    }

    // kinfo!("clone: created child task with TID {}", child_tid);

    Ok(child_tid as usize)
}

pub fn execve(
    uptr_path: UString,
    uptr_argv: UArray<UString>,
    uptr_envp: UArray<UString>,
) -> SyscallRet {
    {
        uptr_path.should_not_null()?;

        let path = uptr_path.read()?;

        // crate::kinfo!("execve: {}", path);

        let file = current::with_cwd(|cwd| {
            vfs::openat_file(&cwd, &path, FileFlags::dontcare(), &Perm::new(PermFlags::X))
        })?;

        let helper = |uarray: UArray<UString>| -> SysResult<Vec<String>> {
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
        };

        let argv = helper(uptr_argv)?;
        let envp = helper(uptr_envp)?;
        let argv_ref: Vec<&str> = argv.iter().map(|s| s.as_str()).collect();
        let envp_ref: Vec<&str> = envp.iter().map(|s| s.as_str()).collect();

        current::pcb().exec(current::tcb(), file, &argv_ref, &envp_ref)?;
        current::tcb().wake_parent_waiting_vfork();
    }

    current::schedule();

    unreachable!()
}

bitflags! {
    pub struct WaitOptions: usize {
        const WNOHANG   = 1 << 0;
        const WUNTRACED = 1 << 1;
    }
}

pub fn wait4(
    pid: usize,
    uptr_status: usize,
    options: usize,
    _user_rusages: usize,
) -> Result<usize, Errno> {
    let pcb = current::pcb();
    let options = WaitOptions::from_bits(options).unwrap_or(WaitOptions::empty());
    let pid = pid as isize;

    let wait_pid;
    let exit_code: usize;

    if pid == -1 {
        if let Some(result) = pcb.wait_any_child(!options.contains(WaitOptions::WNOHANG))? {
            wait_pid = result.0;
            exit_code = result.1 as usize;
        } else {
            return Ok(usize::MAX);
        }
    } else {
        if let Some(result) = pcb.wait_child(pid as i32, !options.contains(WaitOptions::WNOHANG))? {
            wait_pid = pid as i32;
            exit_code = result as usize;
        } else {
            return Ok(usize::MAX);
        }
    }

    if uptr_status != 0 {
        let status: u32 = (exit_code as u32 & 0xff) << 8; // WEXITSTATUS
        copy_to_user::object(uptr_status, status)?;
    }

    Ok(wait_pid as usize)
}

pub fn exit(code: usize) -> Result<usize, Errno> {
    let tcb = current::tcb();
    tcb.exit(code as u8);

    tcb.wake_parent_waiting_vfork();

    current::schedule();

    unreachable!()
}

pub fn exit_group(code: usize) -> Result<usize, Errno> {
    let pcb = current::pcb();
    pcb.exit(code as u8);

    current::schedule();

    unreachable!()
}

pub fn set_tid_address(tid_address: usize) -> Result<usize, Errno> {
    let tcb = current::tcb();
    tcb.set_tid_address(tid_address);
    Ok(0)
}

pub fn getcwd(ubuf: usize, size: usize) -> SysResult<usize> {
    let cwd = current::with_cwd(|dentry| dentry.get_path());
    copy_to_user::string(ubuf, &cwd, size)
}

pub fn chdir(user_path: usize) -> SysResult<usize> {
    let path = copy_from_user::string(user_path)?;
    let dentry = current::with_cwd(|cwd| vfs::load_dentry_at(&cwd, &path))?;
    current::pcb().set_cwd(&dentry);
    Ok(0)
}
