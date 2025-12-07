use num_enum::TryFromPrimitive;
use alloc::vec;

use crate::fs::vfs;
use crate::kernel::scheduler::current;
use crate::kernel::config;
use crate::kernel::errno::Errno;
use crate::kernel::syscall::uptr::{UserPointer, UBuffer, UPtr};
use crate::kernel::syscall::{SyscallRet, UserStruct};
use crate::kernel::uapi;
use crate::klib::random::random;
use crate::arch;

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

impl UserStruct for Utsname {}

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

impl UserStruct for RLimit {}

#[repr(usize)]
#[derive(TryFromPrimitive)]
enum RLimitResource {
    STACK = 3,
    NOFILE = 7,
}

pub fn prlimit64(_pid: usize, resource: usize, uptr_new_limit: UPtr<RLimit>, uptr_old_limit: UPtr<RLimit>) -> SyscallRet {
    // crate::kinfo!("prlimit64: pid={}, resource={}, uptr_new={}, uptr_old={}", _pid, resource, uptr_new_limit.uaddr(), uptr_old_limit.uaddr());
    
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
            }
        }

        RLimitResource::NOFILE => {
            let mut fdtable = current::fdtable().lock();
            if !uptr_old_limit.is_null() {
                let old_limit = RLimit {
                    rlim_cur: fdtable.get_max_fd(),
                    rlim_max: fdtable.get_max_fd(),
                };
                uptr_old_limit.write(old_limit)?;
            }

            if !uptr_new_limit.is_null() {
                let new_limit = uptr_new_limit.read()?;
                if new_limit.rlim_cur != new_limit.rlim_max || new_limit.rlim_max > config::MAX_FD {
                    return Err(Errno::EINVAL);
                }

                fdtable.set_max_fd(new_limit.rlim_max);
            }
        }
    }

    Ok(0)
}

pub fn getrandom(ubuf: UBuffer, len: usize, _flags: usize) -> SyscallRet {
    ubuf.should_not_null()?;
    
    let mut buf = vec![0u8; len];
    for chunk in buf.chunks_mut(4) {
        let rand = random();
        for (i, byte) in chunk.iter_mut().enumerate() {
            *byte = ((rand >> (i * 4)) & 0xFF) as u8;
        }
    }

    ubuf.write(0, &buf)?;

    Ok(len)
}

pub fn membarrier() -> SyscallRet {
    Ok(0)
}

pub fn get_mempolicy() -> SyscallRet {
    // TODO: Not implemented yet
    Ok(0)
}

#[repr(C)]
#[derive(Debug, Copy, Clone)]
pub struct Rusage {
    ru_utime: uapi::TimeVal, // user CPU time used
    ru_stime: uapi::TimeVal, // system CPU time used
    ru_maxrss: isize,      // maximum resident set size
    ru_ixrss: isize,       // integral shared memory size
    ru_idrss: isize,       // integral unshared data size
    ru_isrss: isize,       // integral unshared stack size
    ru_minflt: isize,      // page reclaims (soft page faults)
    ru_majflt: isize,      // page faults (hard page faults)
    ru_nswap: isize,       // swaps
    ru_inblock: isize,     // block input operations
    ru_oublock: isize,     // block output operations
    ru_msgsnd: isize,      // IPC messages sent
    ru_msgrcv: isize,      // IPC messages received
    ru_nsignals: isize,    // signals received
    ru_nvcsw: isize,       // voluntary context switches
    ru_nivcsw: isize,      // involuntary context switches
}

impl UserStruct for Rusage {}

impl Default for Rusage {
    fn default() -> Self {
        Rusage {
            ru_utime: uapi::TimeVal { tv_sec: 0, tv_usec: 0 },
            ru_stime: uapi::TimeVal { tv_sec: 0, tv_usec: 0 },
            ru_maxrss: 0,
            ru_ixrss: 0,
            ru_idrss: 0,
            ru_isrss: 0,
            ru_minflt: 0,
            ru_majflt: 0,
            ru_nswap: 0,
            ru_inblock: 0,
            ru_oublock: 0,
            ru_msgsnd: 0,
            ru_msgrcv: 0,
            ru_nsignals: 0,
            ru_nvcsw: 0,
            ru_nivcsw: 0,
        }
    }
}

#[repr(usize)]
#[derive(TryFromPrimitive, Debug)]
pub enum RusageWho {
    SELF = 0,
}

pub fn getrusage(who: usize, uptr_rusage: UPtr<Rusage>) -> SyscallRet {
    let who = RusageWho::try_from(who).map_err(|_| Errno::EINVAL)?;
    
    let mut rusage = Rusage::default();
    
    match who {
        RusageWho::SELF => {
            let (utime, stime) = current::pcb().tasks_usage_time();
            rusage.ru_utime = utime.into();
            rusage.ru_stime = stime.into();
        }
    };

    // crate::kinfo!("getrusage: who={:?}, rusage={:?}", who, rusage);
    
    uptr_rusage.write(rusage)?;

    Ok(0)
}

pub fn sync() -> SyscallRet {
    vfs::sync_all().map(|_| 0)
}
