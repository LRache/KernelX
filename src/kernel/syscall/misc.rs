use num_enum::TryFromPrimitive;

use crate::kernel::config;
use crate::kernel::errno::Errno;
use crate::kernel::syscall::uptr::{UPtr, UserPointer};
use crate::kernel::syscall::SyscallRet;
use crate::kernel::task::Pid;
use crate::{arch, kinfo};

pub fn set_robust_list() -> Result<usize, Errno> {
    // This syscall is a no-op in the current implementation.
    // It is provided for compatibility with the Linux API.
    Ok(0)
}

pub fn rseq() -> Result<usize, Errno> {
    // This syscall is a no-op in the current implementation.
    // It is provided for compatibility with the Linux API.
    Ok(0)
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct Utsname {
    pub sysname: [u8; 65],
    pub nodename: [u8; 65],
    pub release: [u8; 65],
    pub version: [u8; 65],
    pub machine: [u8; 65],
    pub domainname: [u8; 65],
}

impl Utsname {
    pub fn new() -> Self {
        let mut ustname = Utsname {
            sysname: [0; 65],
            nodename: [0; 65],
            release: [0; 65],
            version: [0; 65],
            machine: [0; 65],
            domainname: [0; 65],
        };
        let sysname = b"KernelX";
        ustname.sysname[..sysname.len()].copy_from_slice(sysname);

        // let release = option_env!("KERNELX_RELEASE").unwrap_or("0.1.0");
        let release = "5.0.0" ;
        ustname.release[..release.len()].copy_from_slice(release.as_bytes());

        let machine = b"riscv64";
        ustname.machine[..machine.len()].copy_from_slice(machine);

        ustname
    }
}

pub fn newuname(uptr_uname: UPtr<Utsname>) -> Result<usize, Errno> {
    uptr_uname.write(Utsname::new())?;

    Ok(0)
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct RLimit {
    rlim_cur: usize,
    rlim_max: usize,
}

#[repr(usize)]
#[derive(TryFromPrimitive)]
enum RLimitResource {
    STACK = 3
}

pub fn prlimit64(pid: usize, resource: usize, uptr_new_limit: UPtr<RLimit>, uptr_old_limit: UPtr<RLimit>) -> SyscallRet {
    // kinfo!("prlimit64: pid={}, resource={}, uptr_new={}, uptr_old={}", pid, resource, uptr_new_limit.uaddr(), uptr_old_limit.uaddr());
    
    let resource = RLimitResource::try_from(resource).map_err(|_| Errno::EINVAL)?;

    match resource {
        RLimitResource::STACK => {
            if !uptr_old_limit.is_null() {
                let stack_size = config::USER_STACK_PAGE_COUNT_MAX * arch::PGSIZE;
                let old_limit = RLimit {
                    rlim_cur: stack_size,
                    rlim_max: stack_size,
                };
                uptr_old_limit.write(old_limit)?;
            }

            if !uptr_new_limit.is_null() {
                let new_limit = uptr_new_limit.read()?;
                if new_limit.rlim_cur != new_limit.rlim_max {
                    return Err(Errno::EINVAL);
                }
                // current::tcb().set_stack_size(new_limit.rlim_cur);
            }
        }
    }

    Ok(0)
}
