use core::usize;
use alloc::vec::Vec;
use bitflags::bitflags;

use crate::fs::file::FileFlags;
use crate::fs::vfs;
use crate::kernel::errno::{Errno, SysResult};
use crate::kernel::scheduler::current::{copy_from_user, copy_to_user};
use crate::kernel::scheduler::current;
use crate::kernel::task::def::TaskCloneFlags;

pub fn sched_yield() -> Result<usize, Errno> {
    current::schedule();
    Ok(0)
}

pub fn getpid() -> Result<usize, Errno> {
    let pcb = current::pcb();
    Ok(pcb.get_pid() as usize)
}

pub fn gettid() -> Result<usize, Errno> {
    let tcb = current::tcb();
    Ok(tcb.get_tid() as usize)
}

bitflags! {
    #[derive(Debug)]
    struct CloneFlags: i32 {
        const CLONE_VM      = 0x00000100;
        const CLONE_FS      = 0x00000200;
        const CLONE_FILES   = 0x00000400;
        const CLONE_SIGHAND = 0x00000800;
        const CLONE_THREAD  = 0x00010000;
        const CLONE_CHILD_CLEARTID = 0x00200000;
        const CLONE_CHILD_SETTID = 0x01000000;
    }
}

pub fn clone(flags: usize, stack: usize, _uptr_parent_tid: usize, _tls: usize, uptr_child_tid: usize) -> Result<usize, Errno> {
    let flags = CloneFlags::from_bits((flags & !0xff) as i32).ok_or(Errno::EINVAL)?;

    let task_flags = TaskCloneFlags {
        vm: flags.contains(CloneFlags::CLONE_VM),
        files: flags.contains(CloneFlags::CLONE_FILES),
        thread: flags.contains(CloneFlags::CLONE_THREAD),
    };

    let child = current::pcb().clone_task(&current::tcb(), stack, &task_flags)?;

    if flags.contains(CloneFlags::CLONE_CHILD_SETTID) {
        child.set_tid_address(uptr_child_tid);
    }

    Ok(child.get_tid() as usize)
}

pub fn execve(uptr_path: usize, uptr_argv: usize, uptr_envp: usize) -> Result<usize, Errno> {
    // let path = current::get_user_string(user_path)?;
    let path = copy_from_user::string(uptr_path)?;
    let file = current::with_cwd(|cwd| vfs::openat_file(&cwd, &path, FileFlags::dontcare()))?;

    let mut argv = Vec::new();
    let mut envp = Vec::new();
        
    if uptr_argv != 0 {
        let mut argv_ptr = uptr_argv;
        loop {
            let ptr: usize = copy_from_user::object(argv_ptr)?;
            if ptr == 0 {
                break;
            }
            // let arg = current::get_user_string(ptr)?;
            let arg = copy_from_user::string(ptr)?;
            argv.push(arg);
            argv_ptr += core::mem::size_of::<usize>();
        }
    }
        
    if uptr_envp != 0 {
        let mut envp_ptr = uptr_envp;
        loop {
            // let ptr = current::copy_from_user::<usize>(envp_ptr)?;
            let ptr: usize = copy_from_user::object(envp_ptr)?;
            if ptr == 0 {
                break;
            }
            let env = copy_from_user::string(ptr)?;
            envp.push(env);
            envp_ptr += core::mem::size_of::<usize>();
        }
    }

    let argv_ref: Vec<&str> = argv.iter().map(|s| s.as_str()).collect();
    let envp_ref: Vec<&str> = envp.iter().map(|s| s.as_str()).collect();

    current::pcb().exec(&current::tcb(), file, &argv_ref, &envp_ref)?;

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
        // copy_to_user!(user_status, status)?;
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
    // copy_to_user_string!(ubuf, cwd, size)
    // current::copy_to_user_string(ubuf, &cwd, size)
    copy_to_user::string(ubuf, &cwd, size)
}

pub fn chdir(user_path: usize) -> SysResult<usize> {
    let path = copy_from_user::string(user_path)?;
    let dentry = current::with_cwd(|cwd| vfs::load_dentry_at(&cwd, &path))?;
    current::pcb().set_cwd(&dentry);
    Ok(0)
}
