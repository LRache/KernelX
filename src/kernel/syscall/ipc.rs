use alloc::sync::Arc;
use alloc::vec::Vec;
use bitflags::bitflags;
use core::convert::TryInto;
use core::time::Duration;
use num_enum::TryFromPrimitive;

use crate::arch;
use crate::kernel::errno::Errno;
use crate::kernel::event::{Event, timer};
use crate::kernel::ipc::shm::{IPC_RMID, IPC_SET, IPC_STAT, IpcFlag, ShmGetFlag, ShmMode};
use crate::kernel::ipc::{
    KSiFields, Pipe, SiCode, SignalAction, SignalNum, SignalSet, SignalStackFlags, SocketType, UnixSocket, shm, signum,
};
use crate::kernel::scheduler::{Tid, current};
use crate::kernel::syscall::UserStruct;
use crate::kernel::syscall::uptr::{UArray, UPtr, UserPointer};
use crate::kernel::task::fdtable::FDFlags;
use crate::kernel::task::{PCB, manager};
use crate::kernel::uapi::OpenFlags;
use crate::kernel::{config, uapi};
use crate::net::socket::{AddressFamily, SOCK_CLOEXEC, SOCK_NONBLOCK, SocketKind};

use super::SyscallRet;

bitflags! {
    struct PipeFlags: usize {
        const O_NONBLOCK = OpenFlags::O_NONBLOCK.bits();
        const O_CLOEXEC = OpenFlags::O_CLOEXEC.bits();
    }
}

const IPPROTO_TCP: usize = 6;
const IPPROTO_UDP: usize = 17;

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
        let mut fdtable = current::fdtable().lock();
        read_fd = fdtable.push(read_end, fd_flags)?;
        write_fd = fdtable.push(write_end, fd_flags)?;
    }

    uptr_pipefd.write(0, &[read_fd as i32, write_fd as i32])?;

    Ok(0)
}

fn unix_socketpair_type(sock_kind: SocketKind, protocol: usize) -> Result<SocketType, Errno> {
    if protocol != 0 {
        return Err(Errno::EPROTONOSUPPORT);
    }

    match sock_kind {
        SocketKind::Stream => Ok(SocketType::Stream),
        SocketKind::Dgram => Ok(SocketType::Dgram),
        SocketKind::SeqPacket => Ok(SocketType::SeqPacket),
        SocketKind::Raw => Err(Errno::EPROTONOSUPPORT),
    }
}

fn inet_socketpair_error(sock_kind: SocketKind, protocol: usize) -> Errno {
    match sock_kind {
        SocketKind::Dgram => match protocol {
            0 | IPPROTO_UDP => Errno::EOPNOTSUPP,
            _ => Errno::EPROTONOSUPPORT,
        },
        SocketKind::Stream => match protocol {
            0 | IPPROTO_TCP => Errno::EOPNOTSUPP,
            _ => Errno::EPROTONOSUPPORT,
        },
        SocketKind::Raw => Errno::EPROTONOSUPPORT,
        SocketKind::SeqPacket => Errno::EINVAL,
    }
}

pub fn socketpair(domain: usize, sock_type: usize, protocol: usize, uptr_sv: UArray<i32>) -> SyscallRet {
    let flags = sock_type & (SOCK_NONBLOCK | SOCK_CLOEXEC);
    let base_type = sock_type & !(SOCK_NONBLOCK | SOCK_CLOEXEC);
    let domain = AddressFamily::try_from(domain).map_err(|_| Errno::EAFNOSUPPORT)?;
    let sock_kind = SocketKind::try_from(base_type).map_err(|_| Errno::EINVAL)?;

    let socket_type = match domain {
        AddressFamily::Unix => unix_socketpair_type(sock_kind, protocol)?,
        AddressFamily::Inet => return Err(inet_socketpair_error(sock_kind, protocol)),
        _ => return Err(Errno::EAFNOSUPPORT),
    };

    let blocked = flags & SOCK_NONBLOCK == 0;
    let cloexec = flags & SOCK_CLOEXEC != 0;
    let fd_flags = FDFlags { cloexec };

    let (sock_a, sock_b) = UnixSocket::create_pair(socket_type, blocked);
    let sock_a = Arc::new(sock_a);
    let sock_b = Arc::new(sock_b);

    let (fd_a, fd_b);
    {
        let mut fdtable = current::fdtable().lock();
        fd_a = fdtable.push(sock_a, fd_flags)?;
        fd_b = match fdtable.push(sock_b, fd_flags) {
            Ok(fd) => fd,
            Err(err) => {
                let _ = fdtable.take(fd_a);
                return Err(err);
            }
        };
    }

    if let Err(err) = uptr_sv.write(0, &[fd_a as i32, fd_b as i32]) {
        let mut fdtable = current::fdtable().lock();
        let _ = fdtable.take(fd_a);
        let _ = fdtable.take(fd_b);
        return Err(err);
    }

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

fn can_send_signal(target: &PCB, signum: SignalNum) -> bool {
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
        pcb.send_signal(signum, SiCode::SI_USER, fields, None)?;
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
            KSiFields::kill(current::pid(), current::uid()),
            Some(tid),
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
    let supported_bits = IpcFlag::all().bits() | ShmGetFlag::all().bits() | ShmMode::all().bits() as usize;
    if shmflg & !supported_bits != 0 {
        return Err(Errno::EINVAL);
    }
    let get_flags = ShmGetFlag::from_bits_truncate(shmflg);
    let ipc_flags = IpcFlag::from_bits_truncate(shmflg);
    let mode = ShmMode::from_bits_truncate(shmflg as u16);
    let shmid = shm::get_or_create_shm(key, size, ipc_flags, get_flags, mode)?;
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
