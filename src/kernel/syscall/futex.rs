use alloc::vec::Vec;
use bitflags::bitflags;
use core::convert::{TryFrom, TryInto};
use core::time::Duration;
use num_enum::TryFromPrimitive;

use crate::driver::chosen::kclock;
use crate::kernel::errno::Errno;
use crate::kernel::event::{Event, timer};
use crate::kernel::scheduler::current;
use crate::kernel::syscall::SyscallRet;
use crate::kernel::syscall::uptr::{UArray, UPtr, UserPointer, UserStruct};
use crate::kernel::usync::futex::{self, RobustListHead};

use super::common::Timespec;

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
    CmpRequeue = 4,
    WaitBitset = 9,
    WakeBitset = 10,
}

const FUTEX_OP_MASK: usize = 0x7f;
const FUTEX_CLOCK_REALTIME: usize = 0x100;
const FUTEX_WAITV_MAX: usize = 128;

bitflags! {
    #[derive(Clone, Copy)]
    struct FutexWaitvFlags: u32 {
        const FUTEX_32 = 0x02;
        const FUTEX_PRIVATE_FLAG = 0x80;
    }
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct FutexWaitv {
    val: u64,
    uaddr: u64,
    flags: u32,
    __reserved: u32,
}

impl UserStruct for FutexWaitv {}

#[repr(usize)]
#[derive(Clone, Copy, TryFromPrimitive)]
enum FutexWaitvClockId {
    Realtime = 0,
    Monotonic = 1,
}

pub fn futex(
    uaddr: UPtr<i32>,
    futex_op: usize,
    val: usize,
    timeout: UPtr<Timespec>,
    uaddr2: UPtr<()>,
    val3: usize,
) -> SyscallRet {
    uaddr.should_not_null()?;
    if uaddr.uaddr() & 3 != 0 {
        return Err(Errno::EINVAL);
    }

    let op = FutexOp::try_from(futex_op & FUTEX_OP_MASK).map_err(|_| Errno::EINVAL)?;
    match op {
        FutexOp::Wait | FutexOp::WaitBitset => {
            let bitset = if op == FutexOp::WaitBitset {
                val3 as u32
            } else {
                u32::MAX
            };
            if bitset == 0 {
                return Err(Errno::EINVAL);
            }

            let timeout = if let Some(timeout) = timeout.read_optional()? {
                let duration: Duration = timeout.try_into()?;
                if op == FutexOp::WaitBitset {
                    let now = if (futex_op & FUTEX_CLOCK_REALTIME) != 0 {
                        kclock::now()?
                    } else {
                        timer::now()
                    };
                    Some(duration.checked_sub(now).unwrap_or(Duration::ZERO))
                } else {
                    Some(duration)
                }
            } else {
                None
            };

            let kaddr = uaddr.kaddr()?;
            futex::wait_current(kaddr, val as i32, bitset)?;
            if timeout == Some(Duration::ZERO) {
                let task = current::task().clone();
                futex::cancel_wait_all(&task);
                return Err(Errno::ETIMEDOUT);
            }

            let timer_id = timeout.map(|timeout| timer::add_timer(current::task().clone(), timeout));
            let event = current::block("futex");
            if !matches!(event, Event::Futex) {
                let task = current::task().clone();
                futex::cancel_wait_all(&task);
            }
            if let Some(timer_id) = timer_id
                && event != Event::Timeout
            {
                timer::remove_timer(timer_id);
            }

            match event {
                Event::Futex => Ok(0),
                Event::Timeout => Err(Errno::ETIMEDOUT),
                Event::Signal => Err(Errno::EINTR),
                _ => unreachable!("state.event={:?}", event),
            }
        }
        FutexOp::Wake | FutexOp::WakeBitset => {
            let kaddr = uaddr.kaddr()?;
            let bitset = if op == FutexOp::WakeBitset {
                val3 as u32
            } else {
                u32::MAX
            };
            if bitset == 0 {
                return Err(Errno::EINVAL);
            }
            futex::wake(kaddr, val, bitset)
        }

        FutexOp::REQUEUE | FutexOp::CmpRequeue => {
            uaddr2.should_not_null()?;
            if uaddr2.uaddr() & 3 != 0 {
                return Err(Errno::EINVAL);
            }
            let kaddr = uaddr.kaddr()?;
            let kaddr2 = uaddr2.kaddr()?;
            let wake_count = val;
            let requeue_count = timeout.uaddr();
            let cmp = (op == FutexOp::CmpRequeue).then_some(val3 as i32);
            futex::requeue(kaddr, kaddr2, wake_count, requeue_count, cmp)
        }
    }
}

pub fn futex_waitv(
    waiters: UArray<FutexWaitv>,
    nr_futexes: usize,
    flags: usize,
    timeout: UPtr<Timespec>,
    clockid: usize,
) -> SyscallRet {
    if flags != 0 || nr_futexes == 0 || nr_futexes > FUTEX_WAITV_MAX || waiters.is_null() {
        return Err(Errno::EINVAL);
    }

    let timeout = if timeout.is_null() {
        None
    } else {
        let clockid = FutexWaitvClockId::try_from(clockid).map_err(|_| Errno::EINVAL)?;
        let deadline: Duration = timeout.read()?.try_into()?;
        let now = match clockid {
            FutexWaitvClockId::Realtime => kclock::now()?,
            FutexWaitvClockId::Monotonic => timer::now(),
        };
        Some(deadline.checked_sub(now).unwrap_or(Duration::ZERO))
    };
    let mut futexes = Vec::new();
    for i in 0..nr_futexes {
        let waiter = waiters.index(i).read()?;
        let waiter_flags = FutexWaitvFlags::from_bits(waiter.flags).ok_or(Errno::EINVAL)?;
        if !waiter_flags.contains(FutexWaitvFlags::FUTEX_32) || waiter.__reserved != 0 {
            return Err(Errno::EINVAL);
        }

        let uaddr = usize::try_from(waiter.uaddr).map_err(|_| Errno::EINVAL)?;
        if uaddr == 0 {
            return Err(Errno::EFAULT);
        }
        if uaddr & 3 != 0 {
            return Err(Errno::EINVAL);
        }

        let kaddr = UPtr::<i32>::from_uaddr(uaddr).kaddr()?;
        let expected = u32::try_from(waiter.val).ok().map(|val| val as i32);
        futexes.push((kaddr, expected));
    }

    let cancel_waiters = |_futexes: &[(usize, Option<i32>)]| {
        let task = current::task().clone();
        futex::cancel_wait_all(&task);
    };

    for (index, (kaddr, expected)) in futexes.iter().enumerate() {
        let Some(expected) = expected else {
            cancel_waiters(&futexes[..index]);
            return Err(Errno::EAGAIN);
        };

        if let Err(err) = futex::wait_current_waitv(*kaddr, *expected, u32::MAX, index) {
            cancel_waiters(&futexes[..index]);
            return Err(err);
        }
    }

    if timeout == Some(Duration::ZERO) {
        cancel_waiters(&futexes);
        return Err(Errno::ETIMEDOUT);
    }

    let timer_id = timeout.map(|timeout| timer::add_timer(current::task().clone(), timeout));
    let event = current::block("futex_waitv");
    cancel_waiters(&futexes);
    if let Some(timer_id) = timer_id
        && event != Event::Timeout
    {
        timer::remove_timer(timer_id);
    }

    match event {
        Event::FutexWaitv { index } => Ok(index),
        Event::Timeout => Err(Errno::ETIMEDOUT),
        Event::Signal => Err(Errno::EINTR),
        _ => unreachable!("state.event={:?}", event),
    }
}
