use crate::arch::UserContextTrait;
use crate::driver;
use crate::kernel::errno::Errno;
use crate::kernel::event::timer;
use crate::kernel::ipc::{KSiFields, SiCode, signum};
use crate::kernel::mm::MemAccessType;
use crate::kernel::mm::maparea::MemoryFaultSignal;
use crate::kernel::scheduler::current;
use crate::kernel::syscall;

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

pub fn trap_return() {
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

    #[cfg(feature = "deadlock-detect")]
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

    if tcb.state_dead_to_exited() {
        current::schedule();
    }
}

pub fn timer_interrupt() {
    timer::interrupt();

    let tcb = current::tcb();
    tcb.recive_pending_signal_from_parent();
    tcb.handle_signal();

    if current::has_task() {
        current::schedule();
    }
}

pub fn syscall(num: usize, args: &syscall::Args, ret_arg_value: usize) -> usize {
    let tcb = current::tcb();

    let ret = syscall::syscall(num, args);

    if ret == Err(Errno::EINTR) && syscall::should_restart_on_eintr(num) {
        tcb.push_ucontext_syscall_retreg(Some(ret_arg_value));
        tcb.recive_pending_signal_from_parent();
        tcb.handle_signal();
    }

    let ret = match ret {
        Ok(ret) => ret,
        Err(errno) => -(errno as isize) as usize,
    };

    tcb.user_context().skip_syscall_instruction();

    current::schedule();

    ret
}

pub fn memory_fault(addr: usize, access_type: MemAccessType) {
    let signal = match current::addrspace().try_to_fix_memory_fault(addr, access_type) {
        Ok(_) => return,
        Err(MemoryFaultSignal::Segv) => signum::SIGSEGV,
        Err(MemoryFaultSignal::Bus) => signum::SIGBUS,
    };

    // TODO: Implement the sicode and fields for memory fault
    current::pcb()
        .send_signal(signal, SiCode::SI_KERNEL, 0, KSiFields::Empty, None)
        .unwrap();
    current::schedule();
}

pub fn illegal_inst() {
    // TODO: Implement the sicode and fields for illegal inst
    current::pcb()
        .send_signal(signum::SIGSEGV, SiCode::SI_KERNEL, 0, KSiFields::Empty, None)
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
