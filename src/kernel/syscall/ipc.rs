use alloc::sync::Arc;
use num_enum::TryFromPrimitive;
use core::time::Duration;
use bitflags::bitflags;

use crate::kernel::config;
use crate::kernel::event::{timer, Event};
use crate::kernel::ipc::{KSiFields, Pipe, SiCode, SignalSet};
use crate::kernel::ipc::shm::{IpcGetFlag, IPC_RMID, IPC_SET, IPC_STAT};
use crate::kernel::ipc::shm;
use crate::kernel::scheduler::{current, Tid};
use crate::kernel::syscall::UserStruct;
use crate::kernel::syscall::uptr::{UserPointer, UArray, UPtr};
use crate::kernel::task::fdtable::FDFlags;
use crate::kernel::errno::Errno;
use crate::kernel::uapi;
use crate::kernel::task::manager;
use crate::arch;

use super::SyscallRet;

bitflags! {
    struct PipeFlags: usize {
        const O_NONBLOCK = 0x4000;
        const O_CLOEXEC = 0x80000;
    }
}

pub fn pipe(uptr_pipefd: UArray<i32>, flags: usize) -> SyscallRet {
    let flags = PipeFlags::from_bits_truncate(flags);
    let fd_flags = FDFlags{
        cloexec: flags.contains(PipeFlags::O_CLOEXEC),
    };
    
    let blocked = !flags.contains(PipeFlags::O_NONBLOCK);
    let (read_end, write_end) = Pipe::create(config::PIPE_CAPACITY, blocked);
    let read_end = Arc::new(read_end);
    let write_end = Arc::new(write_end);

    let (read_fd, write_fd);
    {
        let mut fdtable = current::fdtable().lock();
        read_fd = fdtable.push(read_end, fd_flags)?;
        write_fd = fdtable.push(write_end, fd_flags)?;
    }

    uptr_pipefd.write(0, &[read_fd as i32, write_fd as i32])?;

    Ok(0)
}

pub fn kill(pid: usize, signum: usize) -> SyscallRet {
    let pid = pid as i32;
    let signum = signum as u32;
    
    if pid > 0 {
        let tcb = manager::get(pid).ok_or(Errno::ESRCH)?;
        tcb.parent().send_signal(
            signum.try_into()?, 
            SiCode::SI_USER,
            KSiFields::kill(current::pid(), current::uid()), 
            None
        )?;
    }

    Ok(0)
}

pub fn tkill(tid: usize, signum: usize) -> SyscallRet {
    let tid = tid as Tid;
    let signum = (signum as u32).try_into()?;
    let tcb = manager::get(tid).ok_or(Errno::ESRCH)?;
    tcb.parent().send_signal(
        signum,
        SiCode::SI_TKILL,
        KSiFields::kill(current::pid(), current::uid()),
        Some(tid)
    )?;
    
    Ok(0)
}

pub fn tgkill(tgid: usize, tid: usize, signum: usize) -> SyscallRet {
    let tgid = tgid as i32;
    let tid = tid as i32;
    let signum = signum as u32;

    if tgid >= 0 {
        let tcb = manager::get(tgid).ok_or(Errno::ESRCH)?;
        tcb.parent().send_signal(
            signum.try_into()?,
            SiCode::SI_TKILL,
            KSiFields::kill(current::pid(), current::uid()),
            Some(tid)
        )?;
    }

    Ok(0)
}

#[repr(usize)]
#[derive(Debug, TryFromPrimitive)]
enum SigProcmaskHow {
    Block = 0,
    Unblock = 1,
    Setmask = 2,
}

pub fn rt_sigprocmask(how: usize, uptr_set: UPtr<SignalSet>, uptr_oldset: UPtr<SignalSet>) -> SyscallRet {
    let how = SigProcmaskHow::try_from(how).map_err(|_| Errno::EINVAL)?;
    
    let mut signal_mask = current::tcb().signal_mask.lock();
    if !uptr_oldset.is_null() {
        uptr_oldset.write(*signal_mask)?;
    }
    
    if !uptr_set.is_null() {
        let set = uptr_set.read()?;
        *signal_mask = match how {
            SigProcmaskHow::Block => *signal_mask | set,
            SigProcmaskHow::Unblock => *signal_mask & !set,
            SigProcmaskHow::Setmask => set,
        };
    }
    
    Ok(0)
}

pub fn rt_sigaction(signum: usize, uptr_act: UPtr<uapi::Sigaction>, uptr_oldact: UPtr<uapi::Sigaction>, sigsetsize: usize) -> SyscallRet {
    assert!(sigsetsize == core::mem::size_of::<SignalSet>());

    let signum = (signum as u32).try_into()?;

    let mut signal_actions = current::signal_actions().lock();
    if !uptr_oldact.is_null() {
        let old_action = signal_actions.get(signum);
        uptr_oldact.write(old_action.into())?;
    }

    if !uptr_act.is_null() {
        let new_action = uptr_act.read()?;
        let new_action = new_action.try_into()?;
        
        signal_actions.set(signum, &new_action)?;
    }

    Ok(0)
}

#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct USignalStack {
    ss_sp: usize,
    ss_flags: usize,
    ss_size: usize,
}
impl UserStruct for USignalStack {}

bitflags! {
    struct SignalStackFlags: usize {
        const SS_ONSTACK = 1 << 0;
        const SS_DISABLE = 1 << 1;
    }
}

pub fn sigaltstack(uptr_ss: UPtr<USignalStack>, uptr_oss: UPtr<USignalStack>) -> SyscallRet {
    uptr_ss.should_not_null()?;
    
    let mut signal_actions = current::signal_actions().lock();
    if !uptr_oss.is_null() {
        let stack = if let Some((sp, size)) = signal_actions.get_stack() {
            USignalStack {
                ss_sp: sp,
                ss_flags: SignalStackFlags::SS_ONSTACK.bits(),
                ss_size: size,
            }
        } else {
            USignalStack {
                ss_sp: 0,
                ss_flags: SignalStackFlags::SS_DISABLE.bits(),
                ss_size: 0,
            }
        };
        uptr_oss.write(stack)?;
    }

    if let Some(stack) = uptr_ss.read_optional()? {
        let flags = SignalStackFlags::from_bits(stack.ss_flags).ok_or(Errno::EINVAL)?;
        if flags.contains(SignalStackFlags::SS_ONSTACK) {
            return Err(Errno::EINVAL);
        }
        
        let s = if flags.contains(SignalStackFlags::SS_DISABLE) {
            None
        } else {
            Some((stack.ss_sp, stack.ss_size))
        };
        signal_actions.set_stack(s);
    }

    Ok(0)
}

pub fn rt_sigsuspend(mask: UPtr<SignalSet>) -> SyscallRet {
    mask.should_not_null()?;
    
    let set = mask.read()?;
    
    let tcb = current::tcb();
    let mut signal_mask = tcb.signal_mask.lock();
    let old = *signal_mask;
    *signal_mask = set;

    let event = current::block("sigsuspend");

    *tcb.signal_mask.lock() = old;

    match event {
        Event::Signal => Err(Errno::EINTR),
        _ => unreachable!()
    }
}

pub fn rt_sig_return() -> SyscallRet {
    current::tcb().return_from_signal();
    arch::return_to_user();
}

pub fn sigtimedwait(uptr_set: UPtr<SignalSet>, _uptr_info: UPtr<()>, uptr_timeout: UPtr<uapi::Timespec>) -> SyscallRet {
    uptr_set.should_not_null()?;
    
    let timeout = uptr_timeout.read_optional()?;
    let signal_set = uptr_set.read()?;

    let mut state = current::tcb().state().lock();
    if let Some(pending) = state.pending_signal {
        if signal_set.contains(pending.signum) {
            state.pending_signal.take();
            return Ok(pending.signum.into());
        }
    }

    if let Some(ts) = timeout {
        let timeout_duration = Duration::new(ts.tv_sec as u64, ts.tv_nsec as u32);
        timer::add_timer(current::task().clone(), timeout_duration);
    }

    state.signal_to_wait = signal_set;

    drop(state);

    let event = current::block("sigtimedwait");

    match event {
        Event::WaitSignal { signum } => Ok(signum.into()),
        Event::Signal => Err(Errno::EINTR),
        Event::Timeout => Err(Errno::EAGAIN),
        _ => unreachable!(),
    }
}

pub fn shmget(key: usize, size: usize, shmflg: usize) -> SyscallRet {
    let flags = IpcGetFlag::from_bits_truncate(shmflg);
    let shmid = shm::get_or_create_shm(key, size, flags)?;
    Ok(shmid)
}

pub fn shmat(shmid: usize, shmaddr: usize, shmflg: usize) -> SyscallRet {
    let addr_space = current::addrspace();
    let flags = shm::ShmFlag::from_bits_truncate(shmflg);
    let addr = shm::attach_shm(shmid, addr_space, shmaddr, flags)?;
    Ok(addr)
}

pub fn shmctl(shmid: usize, cmd: usize, _buf: usize) -> SyscallRet {
    match cmd {
        IPC_RMID => {
            shm::mark_remove_shm(shmid)?;
            Ok(0)
        }
        IPC_STAT => {
            // TODO: Implement IPC_STAT
            Err(Errno::ENOSYS)
        }
        IPC_SET => {
            // TODO: Implement IPC_SET
            Err(Errno::ENOSYS)
        }
        _ => Err(Errno::EINVAL),
    }
}

pub fn shmdt(_shmaddr: usize) -> SyscallRet {
    // TODO: Implement shmdt based on address
    // Currently our manager uses shmid to detach, but syscall uses address.
    // We need to find shmid from address or change manager to support detach by address.
    // For now, return ENOSYS or implement a lookup.
    
    // Since we don't have reverse lookup yet, let's leave it as TODO or 
    // we can iterate over areas in addrspace to find the shm area?
    // But shm_manager needs shmid.
    
    // Real implementation would look up the VMA at shmaddr, check if it's a SHM VMA,
    // get the shmid/shm object from it, and then detach.
    
    Err(Errno::ENOSYS)
}
