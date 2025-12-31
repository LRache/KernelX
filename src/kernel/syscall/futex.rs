use num_enum::TryFromPrimitive;

use crate::kernel::event::timer;
use crate::kernel::scheduler::current;
use crate::kernel::uapi;
use crate::kernel::errno::Errno;
use crate::kernel::syscall::uptr::{UserPointer, UPtr};
use crate::kernel::syscall::SyscallRet;
use crate::kernel::usync::futex::{self, RobustListHead};
use crate::kernel::event::Event;

pub fn set_robust_list(u: UPtr<RobustListHead>) -> SyscallRet {
    current::tcb().set_robust_list(u.uaddr());
    Ok(0)
}

pub fn get_robust_list() -> SyscallRet {
    match current::tcb().get_robust_list() {
        Some(robust_list) => Ok(robust_list),
        None => Ok(0),
    }
}


#[repr(usize)]
#[derive(Debug, PartialEq, Eq, PartialOrd, Ord, TryFromPrimitive)]
enum FutexOp {
    Wait = 0,
    Wake = 1,
    REQUEUE = 3,
    WaitBitset = 9,
    WakeBitset = 10,
}

const FUTEX_OP_MASK: usize = 0x7f;

pub fn futex(
    uaddr: UPtr<i32>, 
    futex_op: usize, 
    val: usize, 
    timeout: UPtr<uapi::Timespec>, 
    uaddr2: UPtr<()>, 
    val3: usize
) -> SyscallRet {    
    uaddr.should_not_null()?;
    if uaddr.uaddr() & 3 != 0 {
        return Err(Errno::EINVAL);
    }

    let op = FutexOp::try_from(futex_op & FUTEX_OP_MASK).map_err(|_| Errno::EINVAL)?;
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
                timer::add_timer(current::task().clone(), timeout.into());
            }
            
            let event = current::block("futex");

            match event {
                Event::Futex => {
                    Ok(0)
                }
                Event::Timeout => {
                    Err(Errno::ETIMEDOUT)
                }
                Event::Signal => {
                    Err(Errno::EINTR)
                }
                _ => unreachable!("state.event={:?}", event)
            }
        },
        FutexOp::Wake | FutexOp::WakeBitset => {
            let kaddr = uaddr.kaddr()?;
            futex::wake(kaddr, val, u32::MAX)
        },

        FutexOp::REQUEUE => {
            let kaddr = uaddr.kaddr()?;
            let kaddr2 = uaddr2.kaddr()?;
            futex::requeue(kaddr, kaddr2, val, None)
        },
    }
}
