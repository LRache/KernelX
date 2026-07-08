use alloc::vec::Vec;
use bitflags::bitflags;
use core::convert::{TryFrom, TryInto};
use core::time::Duration;
use num_enum::TryFromPrimitive;

use crate::arch;
use crate::driver::chosen::kclock;
use crate::kernel::errno::{Errno, SysResult};
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
const FUTEX_WAITV_MAX: usize = 128;

bitflags! {
    #[derive(Clone, Copy)]
    struct FutexOpFlags: usize {
        const PRIVATE = 0x80;
        const CLOCK_REALTIME = 0x100;
    }
}

bitflags! {
    #[derive(Clone, Copy)]
    struct FutexWaitvFlags: u32 {
        const FUTEX_32 = 0x02;
        const FUTEX_PRIVATE_FLAG = 0x80;
    }
}

#[repr(C)]
#[derive(Clone, Copy, UserStruct)]
pub struct FutexWaitv {
    val: u64,
    uaddr: u64,
    flags: u32,
    __reserved: u32,
}

#[repr(usize)]
#[derive(Clone, Copy, TryFromPrimitive)]
enum FutexWaitvClockId {
    Realtime = 0,
    Monotonic = 1,
}

fn read_futex_addr(uaddr: UPtr<i32>, key: futex::FutexKey) -> SysResult<futex::FutexAddr> {
    let value = uaddr.read()?;
    Ok(futex::FutexAddr::new(key, value))
}

fn futex_key(uaddr: UPtr<i32>, private: bool) -> SysResult<futex::FutexKey> {
    if private {
        Ok(futex::FutexKey::private(current::addrspace(), uaddr.uaddr()))
    } else {
        let frame = current::addrspace().get_frame(uaddr.uaddr())?;
        Ok(futex::FutexKey::shared(&frame, uaddr.uaddr() & arch::PGMASK))
    }
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
    let op_flags = FutexOpFlags::from_bits_truncate(futex_op);
    let private = op_flags.contains(FutexOpFlags::PRIVATE);
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
                    let now = if op_flags.contains(FutexOpFlags::CLOCK_REALTIME) {
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

            let key = futex_key(uaddr, private)?;
            let task = current::task().clone();

            let mut futex_manager = futex::manager();
            let addr = read_futex_addr(uaddr, key)?;

            if timeout == Some(Duration::ZERO) {
                futex_manager.touch(addr, val as i32)?;
                return Err(Errno::ETIMEDOUT);
            }

            futex_manager.wait_current(addr, val as i32, bitset)?;
            drop(futex_manager);

            let timer_id = timeout.map(|timeout| timer::add_timer(task.clone(), timeout));
            let event = current::block("futex");
            if !matches!(event, Event::Futex) {
                let mut futex_manager = futex::manager();
                futex_manager.cancel_wait_all(&task);
            }
            if let Some(timer_id) = timer_id {
                if event != Event::Timeout {
                    timer::remove_timer(timer_id);
                }
            }

            match event {
                Event::Futex => Ok(0),
                Event::Timeout => Err(Errno::ETIMEDOUT),
                Event::Signal => Err(Errno::EINTR),
                _ => unreachable!("state.event={:?}", event),
            }
        }
        FutexOp::Wake | FutexOp::WakeBitset => {
            let bitset = if op == FutexOp::WakeBitset {
                val3 as u32
            } else {
                u32::MAX
            };
            if bitset == 0 {
                return Err(Errno::EINVAL);
            }
            let key = futex_key(uaddr, private)?;
            let mut futex_manager = futex::manager();
            futex_manager.wake(key, val, bitset)
        }

        FutexOp::REQUEUE | FutexOp::CmpRequeue => {
            uaddr2.should_not_null()?;
            if uaddr2.uaddr() & 3 != 0 {
                return Err(Errno::EINVAL);
            }
            let key = futex_key(uaddr, private)?;
            let key2 = futex_key(UPtr::<i32>::from_uaddr(uaddr2.uaddr()), private)?;
            let wake_count = val;
            let requeue_count = timeout.uaddr();
            let cmp = (op == FutexOp::CmpRequeue).then_some(val3 as i32);
            let mut futex_manager = futex::manager();
            let addr = read_futex_addr(uaddr, key)?;
            futex_manager.requeue(addr, key2, wake_count, requeue_count, cmp)
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

        let uaddr = UPtr::<i32>::from_uaddr(uaddr);
        let key = futex_key(uaddr, waiter_flags.contains(FutexWaitvFlags::FUTEX_PRIVATE_FLAG))?;
        let expected = u32::try_from(waiter.val).map_err(|_| Errno::EAGAIN)? as i32;
        futexes.push((uaddr, key, expected));
    }

    let task = current::task().clone();

    // Hold the futex manager lock while reading each futex word and deciding
    // whether to link the waiter into its queue. Without that, this lost-wakeup
    // race is possible:
    //
    //   | Task A (waiter)             | Task B (waker)
    // 0 | read futex == expected      |
    // 1 |                             | store futex = new_value
    // 2 |                             | futex_wake()
    // 3 | enqueue and sleep           |
    //
    // The lock makes futex_wake wait until Task A has either fully enqueued
    // all waitv entries, rolled them back, or confirmed a zero-timeout touch.
    let mut futex_manager = futex::manager();
    for (index, (uaddr, key, expected)) in futexes.iter().enumerate() {
        let addr = match read_futex_addr(*uaddr, *key) {
            Ok(addr) => addr,
            Err(err) => {
                if timeout != Some(Duration::ZERO) {
                    futex_manager.cancel_wait_all(&task);
                }
                return Err(err);
            }
        };
        if timeout == Some(Duration::ZERO) {
            futex_manager.touch(addr, *expected)?;
            continue;
        }
        if let Err(err) = futex_manager.wait_current_waitv(addr, *expected, u32::MAX, index) {
            futex_manager.cancel_wait_all(&task);
            return Err(err);
        }
    }

    if timeout == Some(Duration::ZERO) {
        return Err(Errno::ETIMEDOUT);
    }
    drop(futex_manager);

    let timer_id = timeout.map(|timeout| timer::add_timer(task.clone(), timeout));
    let event = current::block("futex_waitv");
    let mut futex_manager = futex::manager();
    futex_manager.cancel_wait_all(&task);
    if let Some(timer_id) = timer_id {
        if event != Event::Timeout {
            timer::remove_timer(timer_id);
        }
    }

    match event {
        Event::FutexWaitv { index } => Ok(index),
        Event::Timeout => Err(Errno::ETIMEDOUT),
        Event::Signal => Err(Errno::EINTR),
        _ => unreachable!("state.event={:?}", event),
    }
}
