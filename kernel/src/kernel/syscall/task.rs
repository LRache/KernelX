use core::usize;
use alloc::vec::Vec;

use bitflags::bitflags;

use crate::fs::file::FileFlags;
use crate::fs::vfs;
use crate::kernel::errno::Errno;
use crate::kernel::scheduler::current;
use crate::kernel::task::def::TaskCloneFlags;
use crate::{copy_to_user, ktrace};

pub fn sched_yield() -> Result<usize, Errno> {
    // current::schedule();
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
    ktrace!("sys_clone: flags: {:#x}, stack: {:#x}, uptr_parent_id: {:#x}, tls: {:#x}, uptr_child_tid: {:#x}", flags, stack, _uptr_parent_tid, _tls, uptr_child_tid);

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

pub fn execve(user_path: usize, user_argv: usize, user_envp: usize) -> Result<usize, Errno> {
    let path = current::get_user_string(user_path)?;
    let file = vfs::open_file(&path, FileFlags::dontcare())?;

    let mut argv = Vec::new();
    let mut envp = Vec::new();

    let addrspace = current::addrspace();
        
    if user_argv != 0 {
        let mut argv_ptr = user_argv;
        loop {
            let mut buf = [0u8; core::mem::size_of::<usize>()];
            addrspace.copy_from_user(argv_ptr, &mut buf)?;
            let ptr = usize::from_le_bytes(buf);
            if ptr == 0 {
                break;
            }
            let arg = addrspace.get_user_string(ptr)?;
            argv.push(arg);
            argv_ptr += core::mem::size_of::<usize>();
        }
    }
        
    if user_envp != 0 {
        let mut envp_ptr = user_envp;
        loop {
            let mut buf = [0u8; core::mem::size_of::<usize>()];
            addrspace.copy_from_user(envp_ptr, &mut buf)?;
            let ptr = usize::from_le_bytes(buf);
            if ptr == 0 {
                break;
            }
            let env = addrspace.get_user_string(ptr)?;
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
        const WNOHANG   = 1 << 1;
        const WUNTRACED = 1 << 2;
    }
}

pub fn wait4(pid: usize, user_status: usize, options: usize, _user_rusages: usize) -> Result<usize, Errno> {
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

    if user_status != 0 {
        copy_to_user!(user_status, exit_code)?;
    }

    Ok(wait_pid as usize)
}

pub fn exit(code: usize) -> Result<usize, Errno> {
    let tcb =current::tcb();
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
