use num_enum::TryFromPrimitive;
use alloc::vec;

use crate::kernel::event::timer;
use crate::kernel::scheduler::current;
use crate::kernel::{config, uapi};
use crate::kernel::errno::Errno;
use crate::kernel::syscall::uptr::{UBuffer, UPtr, UserPointer};
use crate::kernel::syscall::SyscallRet;
use crate::kernel::usync::futex;
use crate::kernel::event::Event;
use crate::klib::random::random;
use crate::arch;
use crate::kinfo;

pub fn set_robust_list() -> SyscallRet {
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

pub fn prlimit64(_pid: usize, resource: usize, uptr_new_limit: UPtr<RLimit>, uptr_old_limit: UPtr<RLimit>) -> SyscallRet {
    // kinfo!("prlimit64: pid={}, resource={}, uptr_new={}, uptr_old={}", _pid, resource, uptr_new_limit.uaddr(), uptr_old_limit.uaddr());
    
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

#[repr(usize)]
#[derive(Debug, PartialEq, Eq, PartialOrd, Ord, TryFromPrimitive)]
enum FutexOp {
    Wait = 0,
    Wake = 1,
    WaitBitset = 9,
    WakeBitset = 10,
}

const FUTEX_OP_MASK: usize = 0x7f;

pub fn futex(uaddr: UPtr<i32>, futex_op: usize, val: usize, timeout: UPtr<uapi::Timespec>, _uaddr2: UPtr<()>, val3: usize) -> SyscallRet {    
    uaddr.should_not_null()?;
    if uaddr.uaddr() & 3 != 0 {
        return Err(Errno::EINVAL);
    }

    let op = FutexOp::try_from(futex_op & FUTEX_OP_MASK).map_err(|_| Errno::EINVAL)?;
    // kinfo!("futex: uaddr={:#x}, futex_op={:?}, val={}, timeout={:?}", uaddr.uaddr(), op, val, timeout);

    match op {
        FutexOp::Wait | FutexOp::WaitBitset => {
            let kaddr = uaddr.kaddr()?;
            
            let bitset = if op == FutexOp::WaitBitset {
                val3 as u32
            } else {
                u32::MAX
            };
            
            futex::wait_current(kaddr, val as i32, bitset)?;
            if let Some(timeout) = timeout.read_optional()? {
                timer::add_timer(current::tcb().clone(), timeout.into());
            }
            
            current::block("futex");
            current::schedule();

            let state = current::tcb().state().lock();
            match state.event {
                Some(Event::Futex) => {
                    Ok(0)
                }
                Some(Event::Timeout) => {
                    Err(Errno::ETIMEDOUT)
                }
                Some(Event::Signal) => {
                    Err(Errno::EINTR)
                }
                _ => unreachable!(),
            }
        },
        FutexOp::Wake | FutexOp::WakeBitset => {
            let kaddr = uaddr.kaddr()?;
            let woken = futex::wake(kaddr, val as usize, u32::MAX)?;
            Ok(woken as usize)
        }
    }
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
