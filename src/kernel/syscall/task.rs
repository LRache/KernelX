use alloc::string::String;
use alloc::vec::Vec;
use bitflags::bitflags;

use crate::fs::file::FileFlags;
use crate::fs::vfs;
use crate::kernel::errno::{Errno, SysResult};
use crate::kernel::scheduler::current::{copy_from_user, copy_to_user};
use crate::kernel::scheduler::current;
use crate::kernel::syscall::SyscallRet;
use crate::kernel::syscall::uptr::{UPtr, UString, UserPointer};
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
        const VM      = 0x00000100;
        const FS      = 0x00000200;
        const FILES   = 0x00000400;
        const SIGHAND = 0x00000800;
        const THREAD  = 0x00010000;
        const NEWNS   = 0x00020000;
        const SYSVSEM = 0x00040000;
        const SETTLS  = 0x00080000;
        const PARENT_SETTID  = 0x00100000;
        const CHILD_CLEARTID = 0x00200000;
        const CHILD_SETTID = 0x01000000;
    }
}

pub fn clone(flags: usize, stack: usize, uptr_parent_tid: UPtr<Tid>, tls: usize, uptr_child_tid: usize) -> SyscallRet {
    let flags = CloneFlags::from_bits((flags & !0xff) as i32).ok_or(Errno::EINVAL)?;

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

    let child = current::pcb().clone_task(&current::tcb(), stack, &task_flags, tls)?;

    if flags.contains(CloneFlags::CHILD_SETTID) {
        child.set_tid_address(uptr_child_tid);
    }

    let child_tid = child.get_tid();

    if flags.contains(CloneFlags::PARENT_SETTID) {
        uptr_parent_tid.write(child_tid)?;
    }

    Ok(child_tid as usize)
}

pub fn execve(uptr_path: UString, uptr_argv: UPtr<UString>, uptr_envp: UPtr<UString>) -> SyscallRet {
    uptr_path.should_not_null()?;

    let path = uptr_path.read()?;
    let file = current::with_cwd(|cwd| vfs::openat_file(&cwd, &path, FileFlags::dontcare()))?;

    let helper = |uarray: UPtr<UString>| -> SysResult<Vec<String>> {
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

    current::pcb().exec(&current::tcb(), file, &argv_ref, &envp_ref)?;

    drop(envp);
    drop(argv);
    drop(path);

    current::schedule();

    unreachable!()
}

bitflags! {
    pub struct WaitOptions: usize {
        const WNOHANG   = 1 << 0;
        const WUNTRACED = 1 << 1;
    }
}

pub fn wait4(pid: usize, uptr_status: usize, options: usize, _user_rusages: usize) -> Result<usize, Errno> {
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
