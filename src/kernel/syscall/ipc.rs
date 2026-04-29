use alloc::sync::Arc;
use alloc::vec::Vec;
use bitflags::bitflags;
use core::convert::TryInto;
use core::time::Duration;
use num_enum::TryFromPrimitive;

use crate::arch;
use crate::kernel::errno::{Errno, SysResult};
use crate::kernel::event::{Event, timer};
use crate::kernel::ipc::shm::{IPC_RMID, IPC_SET, IPC_STAT, IpcGetFlag};
use crate::kernel::ipc::{
    KSiFields, Pipe, SiCode, SigInfo, SignalAction, SignalNum, SignalSet, SignalStackFlags, shm, signum,
};
use crate::kernel::scheduler::{Tid, current};
use crate::kernel::syscall::UserStruct;
use crate::kernel::syscall::uptr::{UArray, UPtr, UserPointer};
use crate::kernel::task::fdtable::FDFlags;
use crate::kernel::task::pidfd::PidFile;
use crate::kernel::task::{PCB, manager};
use crate::kernel::uapi::OpenFlags;
use crate::kernel::{config, uapi};

use super::SyscallRet;
use super::common::Timespec;

bitflags! {
    struct PipeFlags: usize {
        const O_NONBLOCK = OpenFlags::O_NONBLOCK.bits();
        const O_CLOEXEC = OpenFlags::O_CLOEXEC.bits();
    }
}

pub fn pipe(uptr_pipefd: UArray<i32>, flags: usize) -> SyscallRet {
    let flags = PipeFlags::from_bits_truncate(flags);
    let fd_flags = FDFlags {
        cloexec: flags.contains(PipeFlags::O_CLOEXEC),
    };

    let blocked = !flags.contains(PipeFlags::O_NONBLOCK);
    let (read_end, write_end) = Pipe::create(config::PIPE_CAPACITY, blocked);
    let read_end = Arc::new(read_end);
    let write_end = Arc::new(write_end);

    let (read_fd, write_fd);
    {
        let fdtable = current::fdtable();
        let mut fdtable = fdtable.lock();
        read_fd = fdtable.push(read_end, fd_flags)?;
        write_fd = fdtable.push(write_end, fd_flags)?;
    }

    uptr_pipefd.write(0, &[read_fd as i32, write_fd as i32])?;

    Ok(0)
}

fn collect_target_pcbs<F>(mut predicate: F) -> Vec<Arc<PCB>>
where
    F: FnMut(&PCB) -> bool,
{
    let mut targets = Vec::new();
    let tcbs = manager::tcbs().lock();
    for tcb in tcbs.values() {
        let pcb = tcb.parent().clone();
        if !predicate(&pcb) {
            continue;
        }
        if targets.iter().any(|target| Arc::ptr_eq(target, &pcb)) {
            continue;
        }
        targets.push(pcb);
    }
    targets
}

pub(super) fn can_send_signal(target: &PCB, signum: SignalNum) -> bool {
    let caller = current::pcb();
    if caller.euid() == 0 {
        return true;
    }

    let caller_ruid = caller.uid();
    let caller_euid = caller.euid();
    if caller_ruid == target.uid()
        || caller_ruid == target.suid()
        || caller_euid == target.uid()
        || caller_euid == target.suid()
    {
        return true;
    }

    signum == signum::SIGCONT && caller.sid() == target.sid()
}

pub fn kill(pid: usize, signum: usize) -> SyscallRet {
    let pid = pid as i32;
    let signum: SignalNum = (signum as u32).try_into()?;
    let fields = KSiFields::kill(current::pid(), current::uid());
    let targets = match pid {
        1.. => {
            let tcb = manager::get(pid).ok_or(Errno::ESRCH)?;
            let mut targets = Vec::new();
            targets.push(Arc::clone(tcb.parent()));
            targets
        }
        0 => {
            let caller_pgid = current::pcb().pgid();
            crate::kinfo!("kill: sending signal to process group {}", caller_pgid);
            collect_target_pcbs(|pcb| pcb.pgid() == caller_pgid)
        }
        -1 => {
            let caller_pid = current::pid();
            collect_target_pcbs(|pcb| pcb.pid() != manager::INIT_UTASK_TID && pcb.pid() != caller_pid)
        }
        _ => {
            let target_pgid = pid.checked_neg().ok_or(Errno::ESRCH)?;
            collect_target_pcbs(|pcb| pcb.pgid() == target_pgid)
        }
    };

    if targets.is_empty() {
        return Err(Errno::ESRCH);
    }

    if signum.is_empty() {
        return if targets.iter().any(|pcb| can_send_signal(pcb, signum)) {
            Ok(0)
        } else {
            Err(Errno::EPERM)
        };
    }

    let mut sent = false;
    for pcb in targets {
        if !can_send_signal(&pcb, signum) {
            continue;
        }
        pcb.send_signal(signum, SiCode::SI_USER, 0, fields, None)?;
        sent = true;
    }

    if sent { Ok(0) } else { Err(Errno::EPERM) }
}

pub fn tkill(tid: usize, signum: usize) -> SyscallRet {
    let tid = tid as Tid;
    if tid <= 0 {
        return Err(Errno::EINVAL);
    }

    let signum: SignalNum = (signum as u32).try_into()?;
    let tcb = manager::get(tid).ok_or(Errno::ESRCH)?;
    if !can_send_signal(tcb.parent(), signum) {
        return Err(Errno::EPERM);
    }
    if !signum.is_empty() {
        tcb.parent().send_signal(
            signum,
            SiCode::SI_TKILL,
            0,
            KSiFields::kill(current::pid(), current::uid()),
            Some(tid),
        )?;
    }

    Ok(0)
}

pub fn tgkill(tgid: usize, tid: usize, signum: usize) -> SyscallRet {
    let tgid = tgid as i32;
    let tid = tid as i32;
    let signum: SignalNum = (signum as u32).try_into()?;
    if tgid <= 0 || tid <= 0 {
        return Err(Errno::EINVAL);
    }

    let tcb = manager::get(tid).ok_or(Errno::ESRCH)?;
    if tcb.parent().pid() != tgid {
        return Err(Errno::ESRCH);
    }
    if !can_send_signal(tcb.parent(), signum) {
        return Err(Errno::EPERM);
    }
    if !signum.is_empty() {
        tcb.parent().send_signal(
            signum,
            SiCode::SI_TKILL,
            0,
            KSiFields::kill(current::pid(), current::uid()),
            Some(tid),
        )?;
    }

    Ok(0)
}

fn pidfd_signal_info(target: &Arc<PCB>, info: UPtr<SigInfo>) -> SysResult<(SiCode, i32, KSiFields)> {
    if info.is_null() {
        return Ok((SiCode::SI_USER, 0, KSiFields::kill(current::pid(), current::uid())));
    }

    let info = info.read()?;
    let self_target = Arc::ptr_eq(target, &current::pcb());
    if !self_target && (info.si_code.0 >= 0 || info.si_code == SiCode::SI_TKILL) {
        return Err(Errno::EPERM);
    }

    let rt = info.rt();
    Ok((
        info.si_code,
        info.si_errno,
        KSiFields::rt(rt.si_pid, rt.si_uid, rt.si_sigval),
    ))
}

pub fn pidfd_send_signal(pidfd: usize, signum: usize, info: UPtr<SigInfo>, flags: usize) -> SyscallRet {
    if flags != 0 {
        return Err(Errno::EINVAL);
    }

    let signum = SignalNum::try_from(signum as u32)?;
    let pidfd = current::fdtable()
        .lock()
        .get(pidfd)?
        .downcast_arc::<PidFile>()
        .map_err(|_| Errno::EBADF)?;
    let target = pidfd.pcb().ok_or(Errno::ESRCH)?;

    if target.is_exited() {
        return Err(Errno::ESRCH);
    }
    if !can_send_signal(&target, signum) {
        return Err(Errno::EPERM);
    }

    let (si_code, si_errno, fields) = pidfd_signal_info(&target, info)?;
    if signum.is_empty() {
        return Ok(0);
    }

    target.send_signal(signum, si_code, si_errno, fields, None)?;
    Ok(0)
}

pub fn rt_sigqueueinfo(pid: usize, signum: usize, info: UPtr<SigInfo>) -> SyscallRet {
    info.should_not_null()?;

    let pid = pid as i32;
    if pid <= 0 {
        return Err(Errno::ESRCH);
    }

    let target_tcb = manager::get(pid).ok_or(Errno::ESRCH)?;
    let target = target_tcb.parent();
    let signum = SignalNum::try_from(signum as u32)?;
    if !can_send_signal(target, signum) {
        return Err(Errno::EPERM);
    }

    let (si_code, si_errno, fields) = pidfd_signal_info(target, info)?;
    if signum.is_empty() {
        return Ok(0);
    }

    target.send_signal(signum, si_code, si_errno, fields, None)?;
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

    let new_set = if uptr_set.is_null() {
        None
    } else {
        Some(uptr_set.read()?.without_unblockable())
    };

    // let mut signal_mask = current::tcb().signal_mask.lock();
    // if !uptr_oldset.is_null() {
    //     uptr_oldset.write(*signal_mask)?;
    // }

    // if let Some(new_set) = new_set {
    //     *signal_mask = match how {
    //         SigProcmaskHow::Block => *signal_mask | new_set,
    //         SigProcmaskHow::Unblock => *signal_mask & !new_set,
    //         SigProcmaskHow::Setmask => new_set,
    //     };
    // }

    let old_set = {
        let mut signal_mask = current::tcb().signal_mask.lock();
        let old_set = *signal_mask;
        if let Some(new_set) = new_set {
            *signal_mask = match how {
                SigProcmaskHow::Block => *signal_mask | new_set,
                SigProcmaskHow::Unblock => *signal_mask & !new_set,
                SigProcmaskHow::Setmask => new_set,
            };
        }
        old_set
        // release lock before writing to user memory
    };

    if !uptr_oldset.is_null() {
        uptr_oldset.write(old_set)?;
    }

    Ok(0)
}

pub fn rt_sigpending(uptr_set: UPtr<SignalSet>, sigsetsize: usize) -> SyscallRet {
    uptr_set.should_not_null()?;
    if sigsetsize != core::mem::size_of::<SignalSet>() {
        return Err(Errno::EINVAL);
    }

    let tcb = current::tcb();
    let mut set = tcb.parent().pending_signals().lock().pending_set(tcb.tid());
    if let Some(pending) = tcb.state().lock().pending_signal {
        set |= pending.signum.to_mask_set();
    }

    uptr_set.write(set)?;
    Ok(0)
}

pub fn rt_sigaction(
    signum: usize,
    uptr_act: UPtr<uapi::Sigaction>,
    uptr_oldact: UPtr<uapi::Sigaction>,
    sigsetsize: usize,
) -> SyscallRet {
    if sigsetsize != core::mem::size_of::<SignalSet>() {
        return Err(Errno::EINVAL);
    }

    let signum: SignalNum = (signum as u32).try_into()?;
    if signum.is_empty() || signum.is_unignorable() {
        return Err(Errno::EINVAL);
    }

    let mut signal_actions = current::signal_actions().lock();
    if !uptr_oldact.is_null() {
        let old_action = signal_actions.get(signum);
        uptr_oldact.write(old_action.into())?;
    }

    if !uptr_act.is_null() {
        let new_action = uptr_act.read()?;
        let mut new_action: SignalAction = new_action.try_into()?;
        new_action.mask = new_action.mask.without_unblockable();

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

const MINSIGSTKSZ: usize = 2048;

pub fn sigaltstack(uptr_ss: UPtr<USignalStack>, uptr_oss: UPtr<USignalStack>) -> SyscallRet {
    let current_tcb = current::tcb();
    let current_stack = current_tcb.get_signal_stack_state();

    if !uptr_oss.is_null() {
        let stack = if let Some((sp, size)) = current_stack.stack {
            let mut flags = SignalStackFlags::empty();
            if current_stack.on_stack {
                flags |= SignalStackFlags::SS_ONSTACK;
            }
            USignalStack {
                ss_sp: sp,
                ss_flags: flags.bits(),
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
        if current_stack.on_stack {
            return Err(Errno::EPERM);
        }

        let flags = SignalStackFlags::from_bits(stack.ss_flags).ok_or(Errno::EINVAL)?;
        if flags.contains(SignalStackFlags::SS_ONSTACK) {
            return Err(Errno::EINVAL);
        }
        if !flags.contains(SignalStackFlags::SS_DISABLE) && stack.ss_size < MINSIGSTKSZ {
            return Err(Errno::ENOMEM);
        }

        let s = if flags.contains(SignalStackFlags::SS_DISABLE) {
            None
        } else {
            Some((stack.ss_sp, stack.ss_size))
        };
        current_tcb.set_signal_stack(s);
    }

    Ok(0)
}

pub fn rt_sigsuspend(mask: UPtr<SignalSet>) -> SyscallRet {
    mask.should_not_null()?;

    let set = mask.read()?;

    let tcb = current::tcb();

    let old = {
        let mut signal_mask = tcb.signal_mask.lock();
        let old = *signal_mask;
        *signal_mask = set;
        old
    };

    tcb.recive_pending_signal_from_parent();
    if tcb.state().lock().pending_signal.is_some() {
        *tcb.signal_mask.lock() = old;
        return Err(Errno::EINTR);
    }

    let event = current::block("sigsuspend");

    *tcb.signal_mask.lock() = old;

    match event {
        Event::Signal => Err(Errno::EINTR),
        _ => unreachable!(),
    }
}

pub fn rt_sig_return() -> SyscallRet {
    current::tcb().return_from_signal();
    arch::return_to_user();
}

pub fn sigtimedwait(uptr_set: UPtr<SignalSet>, _uptr_info: UPtr<()>, uptr_timeout: UPtr<Timespec>) -> SyscallRet {
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
        let timeout_duration: Duration = ts.try_into()?;
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
    let pid = current::pid();
    let flags = shm::ShmFlag::from_bits_truncate(shmflg);
    let addr = shm::attach_shm(shmid, pid, addr_space, shmaddr, flags)?;
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

pub fn shmdt(shmaddr: usize) -> SyscallRet {
    let addr_space = current::addrspace();
    let pid = current::pid();
    shm::detach_shm_by_addr(pid, shmaddr, addr_space)?;
    Ok(0)
}
