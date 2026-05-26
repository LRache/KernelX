use alloc::sync::Arc;
use alloc::vec::Vec;
use bitflags::bitflags;
use core::convert::TryInto;
use core::time::Duration;
use num_enum::TryFromPrimitive;

use crate::arch;
use crate::kernel::errno::{Errno, SysResult};
use crate::kernel::event::{Event, timer};
use crate::kernel::ipc::shm::IpcGetFlag;
use crate::kernel::ipc::{
    KSiFields, PendingSignal, Pipe, SiCode, SigInfo, SignalAction, SignalNum, SignalSet, SignalStackFlags, sem, shm,
    signum,
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

fn siginfo_from_pending_signal(pending: PendingSignal) -> SigInfo {
    let mut info = SigInfo::empty();
    info.si_signo = Into::<u32>::into(pending.signum) as i32;
    info.si_errno = pending.si_errno;
    info.si_code = pending.si_code;
    info.fields = pending.fields.into();
    info
}

fn finish_sigtimedwait(pending: PendingSignal, uptr_info: UPtr<SigInfo>) -> SyscallRet {
    if !uptr_info.is_null() {
        uptr_info.write(siginfo_from_pending_signal(pending))?;
    }

    Ok(pending.signum.into())
}

pub fn sigtimedwait(
    uptr_set: UPtr<SignalSet>,
    uptr_info: UPtr<SigInfo>,
    uptr_timeout: UPtr<Timespec>,
    sigsetsize: usize,
) -> SyscallRet {
    uptr_set.should_not_null()?;
    if sigsetsize != core::mem::size_of::<SignalSet>() {
        return Err(Errno::EINVAL);
    }

    let timeout = uptr_timeout.read_optional()?;
    let signal_set = uptr_set.read()?.without_unblockable();
    let tcb = current::tcb();

    tcb.recive_pending_signal_from_parent();

    if let Some(pending) = {
        let mut state = tcb.state().lock();
        if let Some(pending) = state.pending_signal {
            if signal_set.contains(pending.signum) {
                state.pending_signal.take()
            } else {
                return Err(Errno::EINTR);
            }
        } else {
            None
        }
    } {
        return finish_sigtimedwait(pending, uptr_info);
    }

    if let Some(pending) = tcb.parent().pending_signals().lock().pop_waiting(signal_set, tcb.tid()) {
        return finish_sigtimedwait(pending, uptr_info);
    }

    let timer_id = if let Some(ts) = timeout {
        let timeout_duration: Duration = ts.try_into()?;
        if timeout_duration.is_zero() {
            return Err(Errno::EAGAIN);
        }
        Some(timer::add_timer(current::task().clone(), timeout_duration))
    } else {
        None
    };

    tcb.state().lock().signal_to_wait = signal_set;

    let event = current::block("sigtimedwait");
    tcb.state().lock().signal_to_wait = SignalSet::empty();

    match event {
        Event::WaitSignal { signum } => {
            timer_id.map(|id| timer::remove_timer(id));
            let pending = tcb.state().lock().pending_signal.take();
            if let Some(pending) = pending {
                finish_sigtimedwait(pending, uptr_info)
            } else {
                Ok(signum.into())
            }
        }
        Event::Signal => {
            timer_id.map(|id| timer::remove_timer(id));
            Err(Errno::EINTR)
        }
        Event::Timeout => Err(Errno::EAGAIN),
        _ => unreachable!(),
    }
}

pub fn shmget(key: usize, size: usize, shmflg: usize) -> SyscallRet {
    let flags = IpcGetFlag::from_bits_truncate(shmflg);
    let mode = (shmflg & 0o777) as u32;
    let shmid = shm::get_or_create_shm(key, size, flags, mode, current::euid(), current::egid(), current::pid())?;
    Ok(shmid)
}

pub fn semget(key: usize, nsems: usize, semflg: usize) -> SyscallRet {
    let flags = IpcGetFlag::from_bits_truncate(semflg);
    let mode = (semflg & 0o777) as u32;
    sem::get_or_create_sem(key, nsems, flags, mode, current::euid(), current::egid())
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct UserSembuf {
    sem_num: u16,
    sem_op: i16,
    sem_flg: i16,
}

impl UserStruct for UserSembuf {}

fn read_sem_ops(sops: UArray<UserSembuf>, nsops: usize) -> SysResult<Vec<sem::SemOp>> {
    if sops.is_null() || nsops == 0 {
        return Err(Errno::EINVAL);
    }
    if nsops > sem::limit::MAX_OPS_PER_CALL {
        return Err(Errno::E2BIG);
    }

    let mut ops = Vec::new();
    for i in 0..nsops {
        let op = sops.index(i).read()?;
        ops.push(sem::SemOp {
            sem_num: op.sem_num,
            sem_op: op.sem_op,
            sem_flg: op.sem_flg,
        });
    }

    Ok(ops)
}

fn do_semtimedop(semid: usize, ops: &[sem::SemOp], timeout: Option<Duration>) -> SyscallRet {
    let deadline = timeout.map(|timeout| timer::now() + timeout);

    loop {
        match sem::begin_semop(semid, ops, current::euid(), current::egid(), current::pid())? {
            sem::SemopStatus::Done => return Ok(0),
            sem::SemopStatus::WouldBlock => {}
        }

        let wait_timeout = if let Some(deadline) = deadline {
            let remaining = deadline.checked_sub(timer::now()).unwrap_or(Duration::ZERO);
            if remaining.is_zero() {
                sem::remove_current_waiter(semid);
                return Err(Errno::EAGAIN);
            }
            Some(remaining)
        } else {
            None
        };

        let timer_id = wait_timeout.map(|timeout| timer::add_timer(current::task().clone(), timeout));
        let event = current::block("semop");
        if !matches!(event, Event::Sem) {
            sem::remove_current_waiter(semid);
        }
        if let Some(timer_id) = timer_id {
            if event != Event::Timeout {
                timer::remove_timer(timer_id);
            }
        }

        match event {
            Event::Sem => {}
            Event::Timeout => return Err(Errno::EAGAIN),
            Event::Signal => return Err(Errno::EINTR),
            _ => unreachable!("unexpected event while waiting on semop: {:?}", event),
        }
    }
}

pub fn semop(semid: usize, sops: UArray<UserSembuf>, nsops: usize) -> SyscallRet {
    let ops = read_sem_ops(sops, nsops)?;
    do_semtimedop(semid, &ops, None)
}

pub fn semtimedop(semid: usize, sops: UArray<UserSembuf>, nsops: usize, timeout: UPtr<Timespec>) -> SyscallRet {
    let ops = read_sem_ops(sops, nsops)?;
    let timeout = if timeout.is_null() {
        None
    } else {
        Some(timeout.read()?.try_into()?)
    };
    do_semtimedop(semid, &ops, timeout)
}

pub fn shmat(shmid: usize, shmaddr: usize, shmflg: usize) -> SyscallRet {
    let flags = shm::ShmFlag::from_bits_truncate(shmflg);
    let addrspace = current::addrspace();
    let pid = current::pid();
    let addr = shm::attach_shm(shmid, pid, current::euid(), current::egid(), addrspace, shmaddr, flags)?;
    Ok(addr)
}

#[repr(C)]
#[derive(Clone, Copy)]
struct UserIpcPerm {
    key: i32,
    uid: u32,
    gid: u32,
    cuid: u32,
    cgid: u32,
    mode: u32,
    seq: u16,
    pad2: u16,
    reserved1: usize,
    reserved2: usize,
}

impl UserStruct for UserIpcPerm {}

#[repr(C)]
#[derive(Clone, Copy)]
struct UserShmidDs {
    shm_perm: UserIpcPerm,
    shm_segsz: usize,
    shm_atime: i64,
    shm_dtime: i64,
    shm_ctime: i64,
    shm_cpid: i32,
    shm_lpid: i32,
    shm_nattch: usize,
    reserved5: usize,
    reserved6: usize,
}

impl UserStruct for UserShmidDs {}

impl From<shm::ShmStat> for UserShmidDs {
    fn from(stat: shm::ShmStat) -> Self {
        Self {
            shm_perm: UserIpcPerm {
                key: stat.ds.key as i32,
                uid: stat.ds.uid,
                gid: stat.ds.gid,
                cuid: stat.ds.cuid,
                cgid: stat.ds.cgid,
                mode: stat.ds.mode,
                seq: 0,
                pad2: 0,
                reserved1: 0,
                reserved2: 0,
            },
            shm_segsz: stat.ds.size,
            shm_atime: stat.ds.atime as i64,
            shm_dtime: stat.ds.dtime as i64,
            shm_ctime: stat.ds.ctime as i64,
            shm_cpid: stat.ds.cpid,
            shm_lpid: stat.ds.lpid,
            shm_nattch: stat.nattch,
            reserved5: 0,
            reserved6: 0,
        }
    }
}

#[repr(C)]
#[derive(Clone, Copy)]
struct UserSemidDs {
    sem_perm: UserIpcPerm,
    sem_otime: i64,
    sem_ctime: i64,
    sem_nsems: usize,
    reserved3: usize,
    reserved4: usize,
}

impl UserStruct for UserSemidDs {}

impl From<sem::SemStat> for UserSemidDs {
    fn from(stat: sem::SemStat) -> Self {
        Self {
            sem_perm: UserIpcPerm {
                key: stat.ds.key as i32,
                uid: stat.ds.uid,
                gid: stat.ds.gid,
                cuid: stat.ds.cuid,
                cgid: stat.ds.cgid,
                mode: stat.ds.mode,
                seq: 0,
                pad2: 0,
                reserved1: 0,
                reserved2: 0,
            },
            sem_otime: stat.ds.otime as i64,
            sem_ctime: stat.ds.ctime as i64,
            sem_nsems: stat.ds.nsems,
            reserved3: 0,
            reserved4: 0,
        }
    }
}

#[repr(C)]
#[derive(Clone, Copy)]
struct UserSemInfo {
    semmap: i32,
    semmni: i32,
    semmns: i32,
    semmnu: i32,
    semmsl: i32,
    semopm: i32,
    semume: i32,
    semusz: i32,
    semvmx: i32,
    semaem: i32,
}

impl UserStruct for UserSemInfo {}

impl From<sem::SemInfo> for UserSemInfo {
    fn from(info: sem::SemInfo) -> Self {
        Self {
            semmap: info.semmap,
            semmni: info.semmni,
            semmns: info.semmns,
            semmnu: info.semmnu,
            semmsl: info.semmsl,
            semopm: info.semopm,
            semume: info.semume,
            semusz: info.semusz,
            semvmx: info.semvmx,
            semaem: info.semaem,
        }
    }
}

#[allow(non_camel_case_types)]
#[repr(usize)]
#[derive(Debug, TryFromPrimitive)]
enum SemCtlCmd {
    IPC_RMID = 0,
    IPC_SET = 1,
    IPC_STAT = 2,
    IPC_INFO = 3,
    GETPID = 11,
    GETVAL = 12,
    GETALL = 13,
    GETNCNT = 14,
    GETZCNT = 15,
    SETVAL = 16,
    SETALL = 17,
    SEM_STAT = 18,
    SEM_INFO = 19,
}

bitflags! {
    struct IpcCtlFlags: usize {
        const IPC_64 = 0x0100;
    }
}

pub fn semctl(semid: usize, semnum: usize, cmd: usize, arg: usize) -> SyscallRet {
    let cmd = SemCtlCmd::try_from(cmd & !IpcCtlFlags::IPC_64.bits()).map_err(|_| Errno::EINVAL)?;

    match cmd {
        SemCtlCmd::IPC_RMID => {
            sem::remove_sem(semid, current::euid())?;
            Ok(0)
        }
        SemCtlCmd::IPC_STAT => {
            let uptr_buf = UPtr::<UserSemidDs>::from_uaddr(arg);
            if uptr_buf.is_null() {
                return Err(Errno::EFAULT);
            }
            uptr_buf.write(sem::stat_sem(semid, current::euid(), current::egid())?.into())?;
            Ok(0)
        }
        SemCtlCmd::IPC_SET => {
            let uptr_buf = UPtr::<UserSemidDs>::from_uaddr(arg);
            if uptr_buf.is_null() {
                return Err(Errno::EFAULT);
            }
            let ds = uptr_buf.read()?;
            sem::set_perm_sem(
                semid,
                ds.sem_perm.uid,
                ds.sem_perm.gid,
                ds.sem_perm.mode,
                current::euid(),
            )?;
            Ok(0)
        }
        SemCtlCmd::IPC_INFO => {
            let uptr_buf = UPtr::<UserSemInfo>::from_uaddr(arg);
            if uptr_buf.is_null() {
                return Err(Errno::EFAULT);
            }
            let (highest_index, info) = sem::info_sem(false);
            uptr_buf.write(info.into())?;
            Ok(highest_index)
        }
        SemCtlCmd::GETPID => sem::getpid(semid, semnum, current::euid(), current::egid()),
        SemCtlCmd::GETVAL => sem::getval(semid, semnum, current::euid(), current::egid()),
        SemCtlCmd::GETALL => {
            if arg == 0 {
                return Err(Errno::EFAULT);
            }
            let values = sem::getall(semid, current::euid(), current::egid())?;
            current::copy_to_user::slice(arg, &values)?;
            Ok(0)
        }
        SemCtlCmd::GETNCNT => sem::getncnt(semid, semnum, current::euid(), current::egid()),
        SemCtlCmd::GETZCNT => sem::getzcnt(semid, semnum, current::euid(), current::egid()),
        SemCtlCmd::SETVAL => {
            // SETVAL reads union semun.val, so stale upper bits from other union members are ignored.
            let value = arg as i32;
            sem::setval(semid, semnum, value, current::euid(), current::egid(), current::pid())?;
            Ok(0)
        }
        SemCtlCmd::SETALL => {
            if arg == 0 {
                return Err(Errno::EFAULT);
            }
            let nsems = sem::nsems(semid)?;
            let mut values = alloc::vec![0u16; nsems];
            current::copy_from_user::slice(arg, &mut values)?;
            sem::setall(semid, &values, current::euid(), current::egid(), current::pid())?;
            Ok(0)
        }
        SemCtlCmd::SEM_STAT => {
            let uptr_buf = UPtr::<UserSemidDs>::from_uaddr(arg);
            if uptr_buf.is_null() {
                return Err(Errno::EFAULT);
            }
            let (id, stat) = sem::stat_index_sem(semid, current::euid(), current::egid())?;
            uptr_buf.write(stat.into())?;
            Ok(id)
        }
        SemCtlCmd::SEM_INFO => {
            let uptr_buf = UPtr::<UserSemInfo>::from_uaddr(arg);
            if uptr_buf.is_null() {
                return Err(Errno::EFAULT);
            }
            let (highest_index, info) = sem::info_sem(true);
            uptr_buf.write(info.into())?;
            Ok(highest_index)
        }
    }
}

#[allow(non_camel_case_types)]
#[repr(usize)]
#[derive(Debug, TryFromPrimitive)]
enum ShmCtlCmd {
    IPC_RMID = 0,
    IPC_SET = 1,
    IPC_STAT = 2,
    IPC_INFO = 3,
}

pub fn shmctl(shmid: usize, cmd: usize, buf: usize) -> SyscallRet {
    let cmd = ShmCtlCmd::try_from(cmd).map_err(|_| Errno::EINVAL)?;

    match cmd {
        ShmCtlCmd::IPC_RMID => {
            shm::mark_remove_shm(shmid)?;
            Ok(0)
        }
        ShmCtlCmd::IPC_STAT => {
            let uptr_buf = UPtr::<UserShmidDs>::from_uaddr(buf);
            if uptr_buf.is_null() {
                return Err(Errno::EFAULT);
            }
            uptr_buf.write(shm::stat_shm(shmid)?.into())?;
            Ok(0)
        }
        ShmCtlCmd::IPC_SET => {
            // TODO: Implement IPC_SET
            Err(Errno::ENOSYS)
        }
        ShmCtlCmd::IPC_INFO => Err(Errno::EINVAL),
    }
}

pub fn shmdt(shmaddr: usize) -> SyscallRet {
    let addr_space = current::addrspace();
    let pid = current::pid();
    shm::detach_shm_by_addr(pid, shmaddr, addr_space)?;
    Ok(0)
}
