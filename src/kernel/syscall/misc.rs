use alloc::vec;
use bitflags::bitflags;
use num_enum::TryFromPrimitive;

use crate::fs::vfs;
use crate::kernel::config;
use crate::kernel::errno::Errno;
use crate::kernel::scheduler::current;
use crate::kernel::syscall::uptr::{UBuffer, UPtr, UString, UserPointer};
use crate::kernel::syscall::{SyscallRet, UserStruct};
use crate::kernel::task::{CapabilitySet, UTS_NAME_MAX};
use crate::klib::dmesg;
use crate::klib::random::random;
use crate::{arch, kmodule};

use super::common::Timeval;

const UTS_FIELD_LEN: usize = 65;

pub fn rseq() -> Result<usize, Errno> {
    // This syscall is a no-op in the current implementation.
    // It is provided for compatibility with the Linux API.
    Ok(0)
}

#[repr(C)]
#[derive(Clone, Copy, UserStruct)]
pub struct Utsname {
    pub sysname: [u8; UTS_FIELD_LEN],
    pub nodename: [u8; UTS_FIELD_LEN],
    pub release: [u8; UTS_FIELD_LEN],
    pub version: [u8; UTS_FIELD_LEN],
    pub machine: [u8; UTS_FIELD_LEN],
    pub domainname: [u8; UTS_FIELD_LEN],
}

impl Utsname {
    pub fn new() -> Self {
        let mut ustname = Utsname {
            sysname: [0; UTS_FIELD_LEN],
            nodename: [0; UTS_FIELD_LEN],
            release: [0; UTS_FIELD_LEN],
            version: [0; UTS_FIELD_LEN],
            machine: [0; UTS_FIELD_LEN],
            domainname: [0; UTS_FIELD_LEN],
        };
        // let sysname = b"KernelX";
        let sysname = b"Linux";
        ustname.sysname[..sysname.len()].copy_from_slice(sysname);

        let uts = current::pcb().uts();
        uts.write_hostname_to(&mut ustname.nodename);
        uts.write_domainname_to(&mut ustname.domainname);

        let release = "6.0.0";
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

pub fn sethostname(uptr_name: UBuffer, len: usize) -> SyscallRet {
    if current::pcb().euid() != 0 {
        return Err(Errno::EPERM);
    }

    if len > UTS_NAME_MAX {
        return Err(Errno::EINVAL);
    }

    let mut hostname = [0; UTS_NAME_MAX];
    if len > 0 {
        uptr_name.read(0, &mut hostname[..len])?;
    }

    current::pcb().uts().set_hostname(&hostname[..len]);

    Ok(0)
}

pub fn setdomainname(uptr_name: UBuffer, len: usize) -> SyscallRet {
    if current::pcb().euid() != 0 {
        return Err(Errno::EPERM);
    }

    if len > UTS_NAME_MAX {
        return Err(Errno::EINVAL);
    }

    let mut domainname = [0; UTS_NAME_MAX];
    if len > 0 {
        uptr_name.read(0, &mut domainname[..len])?;
    }

    current::pcb().uts().set_domainname(&domainname[..len]);

    Ok(0)
}

#[repr(C)]
#[derive(Clone, Copy, UserStruct)]
pub struct RLimit {
    rlim_cur: usize,
    rlim_max: usize,
}

#[repr(usize)]
#[derive(Debug, Clone, Copy, TryFromPrimitive)]
enum RLimitResource {
    CPU = 0,
    FSIZE = 1,
    DATA = 2,
    STACK = 3,
    CORE = 4,
    RSS = 5,
    NPROC = 6,
    NOFILE = 7,
    MEMLOCK = 8,
    AS = 9,
    LOCKS = 10,
    SIGPENDING = 11,
    MSGQUEUE = 12,
    NICE = 13,
    RTPRIO = 14,
    RTTIME = 15,
}

pub fn prlimit64(
    _pid: usize,
    resource: usize,
    uptr_new_limit: UPtr<RLimit>,
    uptr_old_limit: UPtr<RLimit>,
) -> SyscallRet {
    let resource = RLimitResource::try_from(resource).map_err(|_| Errno::EINVAL)?;

    match resource {
        RLimitResource::FSIZE => {
            let pcb = current::pcb();
            if !uptr_old_limit.is_null() {
                let (rlim_cur, rlim_max) = pcb.file_size_limit();
                uptr_old_limit.write(RLimit { rlim_cur, rlim_max })?;
            }

            if !uptr_new_limit.is_null() {
                let new_limit = uptr_new_limit.read()?;
                if new_limit.rlim_cur > new_limit.rlim_max {
                    return Err(Errno::EINVAL);
                }

                pcb.set_file_size_limit(new_limit.rlim_cur, new_limit.rlim_max);
            }
        }

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

        // TODO: implement real core dump size limit. For now, just allow unlimited core dump size and ignore any new limit.
        RLimitResource::CORE => {
            crate::kwarn!("prlimit64: RLIMIT_CORE is not fully implemented");

            if !uptr_old_limit.is_null() {
                let old_limit = RLimit {
                    rlim_cur: 0,
                    rlim_max: 0,
                };
                uptr_old_limit.write(old_limit)?;
            }

            if !uptr_new_limit.is_null() {
                // let new_limit = uptr_new_limit.read()?;
                // if new_limit.rlim_cur != 0 || new_limit.rlim_max != 0 {
                //     return Err(Errno::EINVAL);
                // }
            }
        }

        RLimitResource::NOFILE => {
            let fdtable = current::fdtable();
            let mut fdtable = fdtable.lock();
            if !uptr_old_limit.is_null() {
                let old_limit = RLimit {
                    rlim_cur: fdtable.get_max_fd(),
                    rlim_max: fdtable.get_max_fd(),
                };
                uptr_old_limit.write(old_limit)?;
            }

            if !uptr_new_limit.is_null() {
                let new_limit = uptr_new_limit.read()?;
                if new_limit.rlim_max > config::MAX_FD {
                    return Err(Errno::EINVAL);
                }

                // if new_limit.rlim_cur > fdtable.get_max_fd() {
                //     return Err(Errno::EINVAL);
                // }

                fdtable.set_max_fd(new_limit.rlim_max);
            }
        }

        // TODO: implement accounting and enforcement for these resource limits.
        resource @ (RLimitResource::CPU
        | RLimitResource::DATA
        | RLimitResource::RSS
        | RLimitResource::NPROC
        | RLimitResource::MEMLOCK
        | RLimitResource::AS
        | RLimitResource::LOCKS
        | RLimitResource::SIGPENDING
        | RLimitResource::MSGQUEUE
        | RLimitResource::NICE
        | RLimitResource::RTPRIO
        | RLimitResource::RTTIME) => {
            // Warn once per resource; rustc/cargo call this on every process
            // spawn and the repeated serial output floods the console.
            static WARNED: core::sync::atomic::AtomicUsize = core::sync::atomic::AtomicUsize::new(0);
            let bit = 1usize << (resource as usize);
            if WARNED.fetch_or(bit, core::sync::atomic::Ordering::Relaxed) & bit == 0 {
                crate::kwarn!("prlimit64: RLIMIT_{:?} is not implemented", resource);
            }

            if !uptr_old_limit.is_null() {
                let old_limit = RLimit {
                    rlim_cur: usize::MAX,
                    rlim_max: usize::MAX,
                };
                uptr_old_limit.write(old_limit)?;
            }

            if !uptr_new_limit.is_null() {
                let new_limit = uptr_new_limit.read()?;
                if new_limit.rlim_cur > new_limit.rlim_max {
                    return Err(Errno::EINVAL);
                }
            }
        }
    }

    Ok(0)
}

bitflags! {
    struct GetRandomFlags: usize {
        const NONBLOCK = 0x0001;
        const RANDOM = 0x0002;
        const INSECURE = 0x0004;
    }
}

pub fn getrandom(ubuf: UBuffer, len: usize, flags: usize) -> SyscallRet {
    let flags = GetRandomFlags::from_bits(flags).ok_or(Errno::EINVAL)?;
    if flags.contains(GetRandomFlags::RANDOM) && flags.contains(GetRandomFlags::INSECURE) {
        return Err(Errno::EINVAL);
    }

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

fn load_module_status(status: i32) -> SyscallRet {
    if status < 0 {
        let errno = Errno::try_from(-status).unwrap_or(Errno::EINVAL);
        return Err(errno);
    }
    Ok(status as usize)
}

pub fn init_module(uptr_module: UBuffer, len: usize, _uptr_params: UPtr<u8>) -> SyscallRet {
    if !current::capable(CapabilitySet::SYS_MODULE) {
        return Err(Errno::EPERM);
    }
    uptr_module.should_not_null()?;
    if len == 0 {
        return Err(Errno::ENOEXEC);
    }
    if len > kmodule::MAX_KMODULE_IMAGE_SIZE {
        return Err(Errno::EFBIG);
    }

    let mut image = vec![0u8; len];
    uptr_module.read(0, &mut image)?;

    load_module_status(kmodule::load(&image)?)
}

pub fn finit_module(fd: usize, _uptr_params: UPtr<u8>, _flags: usize) -> SyscallRet {
    if !current::capable(CapabilitySet::SYS_MODULE) {
        return Err(Errno::EPERM);
    }

    let file = current::fdtable().lock().get(fd)?;
    load_module_status(kmodule::load_file(file.as_ref())?)
}

pub fn delete_module(uptr_name: UString, _flags: usize) -> SyscallRet {
    if !current::capable(CapabilitySet::SYS_MODULE) {
        return Err(Errno::EPERM);
    }
    uptr_name.should_not_null()?;

    let name = uptr_name.read_string_fixed::<256>()?;
    kmodule::unload(name)?;
    Ok(0)
}

pub fn get_mempolicy() -> SyscallRet {
    // TODO: Not implemented yet
    Ok(0)
}

#[repr(C)]
#[derive(Debug, Copy, Clone, UserStruct)]
pub struct Rusage {
    ru_utime: Timeval,  // user CPU time used
    ru_stime: Timeval,  // system CPU time used
    ru_maxrss: isize,   // maximum resident set size
    ru_ixrss: isize,    // integral shared memory size
    ru_idrss: isize,    // integral unshared data size
    ru_isrss: isize,    // integral unshared stack size
    ru_minflt: isize,   // page reclaims (soft page faults)
    ru_majflt: isize,   // page faults (hard page faults)
    ru_nswap: isize,    // swaps
    ru_inblock: isize,  // block input operations
    ru_oublock: isize,  // block output operations
    ru_msgsnd: isize,   // IPC messages sent
    ru_msgrcv: isize,   // IPC messages received
    ru_nsignals: isize, // signals received
    ru_nvcsw: isize,    // voluntary context switches
    ru_nivcsw: isize,   // involuntary context switches
}

impl Default for Rusage {
    fn default() -> Self {
        Rusage {
            ru_utime: Timeval::ZERO,
            ru_stime: Timeval::ZERO,
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
    CHILDREN = -1isize as usize,
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
        RusageWho::CHILDREN => {
            let (utime, stime) = current::pcb().children_usage_time();
            rusage.ru_utime = utime.into();
            rusage.ru_stime = stime.into();
        }
    };

    uptr_rusage.write(rusage)?;

    Ok(0)
}

#[repr(C)]
#[derive(Default, Debug, Clone, Copy, UserStruct)]
pub struct Sysinfo {
    uptime: usize,
    loads: [usize; 3],
    totalram: usize,
    freeram: usize,
    sharedram: usize,
    bufferram: usize,
    totalswap: usize,
    freeswap: usize,
    procs: u16,
    totalhigh: usize,
    freehigh: usize,
    mem_unit: u32,
    _f: [u8; 20 - 2 * core::mem::size_of::<isize>() - core::mem::size_of::<i32>()],
}

/// Clock ticks per second, matching the Linux userspace CLK_TCK value.
const CLK_TCK: u64 = 100;

#[repr(C)]
#[derive(Default, Clone, Copy, UserStruct)]
pub struct Tms {
    tms_utime: usize,  // user CPU time of process
    tms_stime: usize,  // system CPU time of process
    tms_cutime: usize, // user CPU time of waited-for children
    tms_cstime: usize, // system CPU time of waited-for children
}

pub fn times(uptr_tms: UPtr<Tms>) -> SyscallRet {
    let pcb = current::pcb();
    let (utime, stime) = pcb.tasks_usage_time();
    let (cutime, cstime) = pcb.children_usage_time();

    let tms = Tms {
        tms_utime: (utime.as_millis() as u64 * CLK_TCK / 1000) as usize,
        tms_stime: (stime.as_millis() as u64 * CLK_TCK / 1000) as usize,
        tms_cutime: (cutime.as_millis() as u64 * CLK_TCK / 1000) as usize,
        tms_cstime: (cstime.as_millis() as u64 * CLK_TCK / 1000) as usize,
    };

    uptr_tms.write(tms)?;

    let uptime_ticks = (arch::uptime().as_millis() as u64 * CLK_TCK / 1000) as usize;
    Ok(uptime_ticks)
}

pub fn sysinfo(uptr_sysinfo: UPtr<Sysinfo>) -> SyscallRet {
    uptr_sysinfo.should_not_null()?;
    let mut sysinfo = Sysinfo::default();
    sysinfo.uptime = arch::uptime().as_secs() as usize;
    uptr_sysinfo.write(sysinfo)?;
    Ok(0)
}

pub fn sync() -> SyscallRet {
    vfs::sync_all().map(|_| 0)
}

const PER_LINUX: usize = 0x0000;

pub fn personality(persona: usize) -> SyscallRet {
    // If persona is 0xFFFFFFFF, return the current personality without changing it.
    if persona == 0xFFFFFFFF {
        return Ok(PER_LINUX);
    }
    // Accept PER_LINUX and UNAME26, ignore other flags.
    let base = persona & 0xFF;
    if base != PER_LINUX {
        return Err(Errno::EINVAL);
    }
    // We don't actually store the personality; just return the previous value (PER_LINUX).
    Ok(PER_LINUX)
}

// TODO: implement sched_setaffinity syscall
pub fn sched_setaffinity(_pid: usize, _cpusetsize: usize, _uptr_mask: UBuffer) -> SyscallRet {
    // Pretend success, ignore the mask
    Ok(0)
}

pub fn sched_getaffinity(_pid: usize, cpusetsize: usize, uptr_mask: UBuffer) -> SyscallRet {
    let cpu_count = arch::cpu_count();
    if cpusetsize < cpu_count.div_ceil(u8::BITS as usize) {
        return Err(Errno::EINVAL);
    }

    let mut cpuset_buffer = vec![0u8; cpusetsize];
    for cpu in 0..cpu_count {
        cpuset_buffer[cpu / u8::BITS as usize] |= 1 << (cpu % u8::BITS as usize);
    }

    uptr_mask.write(0, &cpuset_buffer)?;

    Ok(cpusetsize)
}

#[derive(TryFromPrimitive)]
#[repr(usize)]
enum SchedPolicy {
    Other = 0,
    Fifo = 1,
    Rr = 2,
    Batch = 3,
    Idle = 5,
    Deadline = 6,
}

impl SchedPolicy {
    fn priority_max(&self) -> usize {
        match self {
            SchedPolicy::Fifo | SchedPolicy::Rr => 99,
            SchedPolicy::Other | SchedPolicy::Batch | SchedPolicy::Idle | SchedPolicy::Deadline => 0,
        }
    }

    fn priority_min(&self) -> usize {
        match self {
            SchedPolicy::Fifo | SchedPolicy::Rr => 1,
            SchedPolicy::Other | SchedPolicy::Batch | SchedPolicy::Idle | SchedPolicy::Deadline => 0,
        }
    }
}

// TODO: implement real scheduling policy setting
pub fn sched_setscheduler(_pid: usize, _policy: usize, _uptr_param: UPtr<u32>) -> SyscallRet {
    // Pretend success, always using SCHED_OTHER
    Ok(0)
}

// TODO: implement real scheduling policy query
pub fn sched_getscheduler(_pid: usize) -> SyscallRet {
    // Return SCHED_OTHER (0) as the default policy
    Ok(0)
}

// TODO: implement real scheduling parameter query
pub fn sched_getparam(_pid: usize, uptr_param: UPtr<u32>) -> SyscallRet {
    // Write sched_priority = 0 (default for SCHED_OTHER)
    uptr_param.write(0)?;
    Ok(0)
}

pub fn sched_get_priority_max(policy: usize) -> SyscallRet {
    let policy = SchedPolicy::try_from(policy).map_err(|_| Errno::EINVAL)?;
    Ok(policy.priority_max())
}

pub fn sched_get_priority_min(policy: usize) -> SyscallRet {
    let policy = SchedPolicy::try_from(policy).map_err(|_| Errno::EINVAL)?;
    Ok(policy.priority_min())
}

// TODO: implement real reboot syscall
pub fn reboot() -> SyscallRet {
    Ok(0)
}

#[derive(TryFromPrimitive)]
#[repr(usize)]
enum SyslogAction {
    Read = 2,
    ReadAll = 3,
    ReadClear = 4,
    Clear = 5,
    SizeUnread = 9,
    SizeBuffer = 10,
}

pub fn syslog(cmd: usize, ubuf: UBuffer, len: usize) -> SyscallRet {
    match SyslogAction::try_from(cmd).map_err(|_| Errno::EINVAL)? {
        SyslogAction::Read | SyslogAction::ReadAll => {
            ubuf.should_not_null()?;
            let mut buf = vec![0u8; len];
            let n = dmesg::dmesg_read(&mut buf);
            ubuf.write(0, &buf[..n])?;
            Ok(n)
        }
        SyslogAction::ReadClear => {
            ubuf.should_not_null()?;
            let mut buf = vec![0u8; len];
            let n = dmesg::dmesg_read_clear(&mut buf);
            ubuf.write(0, &buf[..n])?;
            Ok(n)
        }
        SyslogAction::Clear => {
            dmesg::dmesg_clear();
            Ok(0)
        }
        SyslogAction::SizeUnread => Ok(dmesg::dmesg_size_unread()),
        SyslogAction::SizeBuffer => Ok(dmesg::dmesg_size_buffer()),
    }
}
