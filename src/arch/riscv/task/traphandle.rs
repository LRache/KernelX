use crate::arch::riscv::cpu::get_cpu_info;
use crate::arch::riscv::csr::scause::Interrupt;
use crate::arch::riscv::csr::*;
use crate::arch::riscv::{UserContext, plic};
use crate::arch::{self, UserContextTrait};
use crate::kernel::mm::MemAccessType;
use crate::kernel::scheduler::current;
use crate::kernel::trap;
use crate::kinfo;

fn handle_syscall() {
    let tcb = current::tcb();

    let user_context = tcb.user_context();

    user_context.gpr[10] = trap::syscall(
        user_context.gpr[17],
        &user_context.gpr[10..17].try_into().unwrap(),
        user_context.gpr[10],
    ) as usize;
}

fn handle_external_interrupt() {
    if let Some(irq) = plic::claim_irq(current::hart_id()) {
        trap::external_interrupt(irq);
        plic::complete_irq(current::hart_id(), irq);
    }
}

pub fn handle_interrupt(interrupt: Interrupt) {
    match interrupt {
        Interrupt::Software => {
            // SAFETY: Clearing SSIP only acknowledges the current hart's
            // supervisor software interrupt and does not access memory.
            unsafe {
                core::arch::asm!("csrc sip, {mask}", mask = in(reg) 1usize << 1, options(nostack, preserves_flags));
            }
        }
        Interrupt::Timer => {
            trap::timer_interrupt();
        }
        Interrupt::External => {
            handle_external_interrupt();
        }
        Interrupt::Counter => {
            kinfo!("Counter interrupt occurred");
        }
    }
}

pub fn save_float_registers(fpregs: &mut [u64; 33]) {
    unsafe extern "C" {
        fn asm_save_float(fpregs: *mut u64);
        fn asm_save_double(fpregs: *mut u64);
    }

    let cpu_info = get_cpu_info(current::hart_id());
    if cpu_info.double_supported() {
        unsafe {
            asm_save_double(fpregs.as_mut_ptr());
        }
    } else if cpu_info.float_supported() {
        unsafe {
            asm_save_float(fpregs.as_mut_ptr());
        }
    }
}

pub fn restore_float_registers(fpregs: &[u64; 33]) {
    unsafe extern "C" {
        fn asm_restore_float(fpregs: *const u64);
        fn asm_restore_double(fpregs: *const u64);
    }

    let cpu_info = get_cpu_info(current::hart_id());
    if cpu_info.double_supported() {
        unsafe {
            asm_restore_double(fpregs.as_ptr());
        }
    } else if cpu_info.float_supported() {
        unsafe {
            asm_restore_float(fpregs.as_ptr());
        }
    }
}

pub fn install_kerneltrap_handler() {
    unsafe extern "C" {
        fn asm_kerneltrap_entry() -> !;
    }
    let stack_top = current::processor().kernel_trap_stack_top();
    assert_ne!(stack_top, 0, "Kernel trap stack is not initialized");
    sscratch::write(stack_top);
    stvec::write(asm_kerneltrap_entry as *const () as usize);
}

fn svadu_mark_page_accessed(uaddr: usize) -> bool {
    let addrspace = current::addrspace();
    let (changed, cpu_mask) = {
        let mut pagetable = addrspace.pagetable().lock();
        let changed = pagetable.mark_page_accessed(uaddr);
        let cpu_mask = if changed { pagetable.active_cpu_mask() } else { 0 };
        (changed, cpu_mask)
    };
    arch::flush_tlb_cpu_mask(cpu_mask);
    changed
}

fn svadu_mark_page_accessed_and_dirty(uaddr: usize) -> bool {
    let addrspace = current::addrspace();
    let (changed, cpu_mask) = {
        let mut pagetable = addrspace.pagetable().lock();
        let changed = pagetable.mark_page_accessed_and_dirty(uaddr);
        let cpu_mask = if changed { pagetable.active_cpu_mask() } else { 0 };
        (changed, cpu_mask)
    };
    arch::flush_tlb_cpu_mask(cpu_mask);
    changed
}

pub fn usertrap_handler() -> ! {
    install_kerneltrap_handler();

    // User execution has stopped. Kernel paths do not directly use user
    // virtual addresses, and the return path performs a local SFENCE.VMA
    // before the address space can be used again.
    current::addrspace().deactivate_cpu(current::hart_id());

    debug_assert!(
        Sstatus::read().sie() == false,
        "Interrupts should be disabled when handling user traps"
    );
    debug_assert!(
        Sstatus::read().spp() == SstatusSPP::User,
        "User trap should come from user mode"
    );
    let user_context = current::tcb().user_context();
    user_context.set_user_entry(sepc::read());

    let cpu_info = get_cpu_info(current::hart_id());
    let sstatus = Sstatus::read();

    if sstatus.fs() != SstatusFs::Off {
        save_float_registers(&mut user_context.fpregs);
        user_context.fpregs_dirty = true;
    }

    trap::trap_enter();

    match scause::cause() {
        scause::Cause::Trap(trap) => match trap {
            scause::Trap::EcallU => {
                handle_syscall();
            }
            scause::Trap::InstPageFault => {
                let addr = stval::read();
                if cpu_info.svadu_enabled() || !svadu_mark_page_accessed(addr) {
                    trap::memory_fault(addr, MemAccessType::Execute);
                }
            }
            scause::Trap::LoadPageFault => {
                let addr = stval::read();
                if cpu_info.svadu_enabled() || !svadu_mark_page_accessed(addr) {
                    trap::memory_fault(addr, MemAccessType::Read);
                }
            }
            scause::Trap::StorePageFault => {
                let addr = stval::read();
                if cpu_info.svadu_enabled() || !svadu_mark_page_accessed_and_dirty(addr) {
                    trap::memory_fault(addr, MemAccessType::Write);
                }
            }
            scause::Trap::IllegalInst => {
                trap::illegal_inst();
            }
            scause::Trap::InstAddrMisaligned | scause::Trap::LoadAddrMisaligned | scause::Trap::StoreAddrMisaligned => {
                trap::memory_misaligned();
            }
            _ => {
                let inst: u32 = current::addrspace().copy_from_user(sepc::read()).unwrap();
                panic!(
                    "Unhandled user trap: {:?}, sepc={:#x}, stval={:#x}, stinst={:#x}, cause={:?}",
                    trap,
                    sepc::read(),
                    stval::read(),
                    inst,
                    scause::cause()
                );
            }
        },

        scause::Cause::Interrupt(interrupt) => handle_interrupt(interrupt),
    }

    if current::tcb().is_exited() {
        current::schedule();
        unreachable!();
    }

    return_to_user();
}

unsafe extern "C" {
    fn asm_usertrap_entry(user_context: *mut UserContext) -> !;
    fn asm_usertrap_return(user_context: *const UserContext) -> !;
}

fn usertrap_return(user_context: &UserContext) -> ! {
    unsafe { asm_usertrap_return(user_context as *const UserContext) }
}

pub fn return_to_user() -> ! {
    trap::trap_return();

    let tcb = current::tcb();

    sepc::write(tcb.user_context().get_user_entry());
    stvec::write(asm_usertrap_entry as *const () as usize);
    sscratch::write(tcb.get_user_context_ptr());

    let user_context = tcb.user_context();
    if user_context.fpregs_dirty {
        restore_float_registers(&user_context.fpregs);
        user_context.fpregs_dirty = false;
    }

    Sstatus::read()
        .set_sie(false)
        .set_spie(true) // Enable interrupts in user mode
        .set_spp(SstatusSPP::User)
        .set_fs(SstatusFs::Clean)
        .write();

    // Publish this CPU under the page-table lock before asm_usertrap_return
    // performs the local SFENCE.VMA and starts using user translations.
    current::addrspace().activate_cpu(current::hart_id());
    usertrap_return(user_context);
}

#[unsafe(no_mangle)]
pub fn kerneltrap_handler() {
    debug_assert!(
        Sstatus::read().sie() == false,
        "Interrupts should be disabled when handling kernel traps"
    );
    debug_assert!(
        Sstatus::read().spp() == SstatusSPP::Supervisor,
        "Interrupts should come from supervisor mode when handling kernel traps"
    );

    let sepc = sepc::read();

    let cause = scause::cause();
    // kinfo!("Kernel trap handler invoked, caused by: {:?}", cause);
    match cause {
        scause::Cause::Trap(trap) => match trap {
            scause::Trap::StorePageFault => {
                let stval = stval::read();
                if current::has_task() {
                    let task = current::task();
                    if task.check_kernel_stack_overflow(stval) {
                        panic!(
                            "Kernel stack overflow detected at address: {:#x}, pc={:#x}, tid={}",
                            stval,
                            sepc,
                            current::tid()
                        );
                    }
                }
                panic!(
                    "Kernel page fault at address: {:#x}, sepc={:#x}, cause={:?}",
                    stval, sepc, trap
                );
            }
            _ => {
                panic!(
                    "Unhandled kernel trap: {:?}, sepc={:#x}, stval={:#x}, cause={:?}",
                    trap,
                    sepc,
                    stval::read(),
                    scause::cause()
                );
            }
        },

        scause::Cause::Interrupt(interrupt) => handle_interrupt(interrupt),
    }

    Sstatus::read().set_spp(SstatusSPP::Supervisor).write();

    sepc::write(sepc);
}
