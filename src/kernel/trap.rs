use crate::arch::UserContextTrait;
use crate::driver;
use crate::kernel::errno::Errno;
use crate::kernel::event::timer;
use crate::kernel::ipc::{KSiFields, SiCode, SiSigFault, signum};
use crate::kernel::mm::MemAccessType;
use crate::kernel::mm::maparea::MemoryFaultError;
use crate::kernel::scheduler::current;
use crate::kernel::syscall;

pub fn handle_signal() -> bool {
    let tcb = current::tcb();
    tcb.recive_pending_signal_from_parent();
    tcb.handle_signal()
}

pub fn trap_enter() {
    let tcb = current::tcb();
    let now = timer::now();
    let mut counter = tcb.time_counter.lock();
    counter.system_start = Some(now);
    let user_start = counter.user_start.take().unwrap();
    let user_delta = now.checked_sub(user_start).unwrap_or_default();
    counter.user_time += user_delta;
    drop(counter);

    tcb.parent().add_task_time(user_delta, core::time::Duration::ZERO);
    tcb.check_cpu_timers();
}

#[cfg(target_arch = "riscv64")]
fn assert_user_satp_matches_addrspace() {
    let tcb = current::tcb();
    let user_satp = tcb.user_context().user_satp;
    let addrspace_satp = tcb.get_addrspace().with_pagetable(|pagetable| pagetable.get_satp());

    // TODO: Remove this diagnostic assert after the RISC-V satp lifetime fix is validated.
    debug_assert_eq!(
        user_satp,
        addrspace_satp,
        "task {} returning to user with stale satp: user_context={:#x}, addrspace={:#x}",
        tcb.tid(),
        user_satp,
        addrspace_satp
    );
}

pub fn trap_return() {
    handle_signal();

    let tcb = current::tcb();
    let now = timer::now();
    let mut counter = tcb.time_counter.lock();
    counter.user_start = Some(now);
    let system_start = counter.system_start.take().unwrap();
    let system_delta = now.checked_sub(system_start).unwrap_or_default();
    counter.system_time += system_delta;
    drop(counter);

    tcb.parent().add_task_time(core::time::Duration::ZERO, system_delta);
    tcb.check_cpu_timers();

    #[cfg(feature = "lockdep")]
    {
        use crate::kernel::scheduler::Task;

        let lockstate = tcb.lockstate();
        debug_assert!(
            lockstate.held().is_empty(),
            "Task {} is returning from trap while still holding locks: {:?}",
            current::tid(),
            lockstate.held()
        );
    }

    tcb.pop_ucontext_syscall_retreg();

    #[cfg(target_arch = "riscv64")]
    assert_user_satp_matches_addrspace();

    if tcb.apply_pending_state_change() {
        current::schedule();
    }
}

pub fn timer_interrupt() {
    timer::interrupt();

    if current::has_task() {
        current::schedule();
    }
}

pub fn syscall(num: usize, args: &syscall::Args, ret_arg_value: usize) -> usize {
    let tcb = current::tcb();

    let ret = syscall::syscall(num, args);

    if ret == Err(Errno::EINTR) && syscall::should_restart_on_eintr(num) {
        tcb.push_ucontext_syscall_retreg(Some(ret_arg_value));
    }

    let ret = match ret {
        Ok(ret) => ret,
        Err(errno) => -(errno as isize) as usize,
    };

    tcb.user_context().skip_syscall_instruction();

    // current::schedule();

    ret
}

pub fn memory_fault(addr: usize, access_type: MemAccessType) {
    let (signal, si_code) = match current::addrspace().try_to_fix_memory_fault(addr, access_type) {
        Ok(()) => return,
        Err(MemoryFaultError::SegvMapError) => (signum::SIGSEGV, SiCode::SEGV_MAPERR),
        Err(MemoryFaultError::SegvAccessError) => (signum::SIGSEGV, SiCode::SEGV_ACCERR),
        Err(MemoryFaultError::BusAddressError) => (signum::SIGBUS, SiCode::BUS_ADRERR),
    };

    if signal == signum::SIGSEGV {
        crate::kdebug!(
            "Memory fault at address: 0x{:x}, access type: {:?}, send SIGSEGV",
            addr,
            access_type
        );
    }

    let fields = KSiFields::SigFault(SiSigFault {
        si_addr: addr,
        si_addr_lsb: 0,
    });
    current::pcb()
        .send_signal(signal, si_code, 0, fields, Some(current::tid()))
        .unwrap();
}

pub fn illegal_inst(pc: usize) {
    let fields = KSiFields::SigFault(SiSigFault {
        si_addr: pc,
        si_addr_lsb: 0,
    });
    current::pcb()
        .send_signal(signum::SIGILL, SiCode::ILL_ILLOPC, 0, fields, Some(current::tid()))
        .unwrap();
}

pub fn memory_misaligned() {
    current::pcb()
        .send_signal(signum::SIGBUS, SiCode::SI_KERNEL, 0, KSiFields::Empty, None)
        .unwrap();
}

pub fn external_interrupt(irq: u32) {
    driver::handle_interrupt(irq);
}
