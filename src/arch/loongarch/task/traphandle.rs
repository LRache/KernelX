use crate::arch::UserContextTrait;
use crate::arch::loongarch::UserContext;
use crate::arch::loongarch::csr;
use crate::arch::loongarch::eiointc;
use crate::kernel::mm::MemAccessType;
use crate::kernel::scheduler::current;
use crate::kernel::trap;
use crate::kwarn;

unsafe extern "C" {
    fn asm_usertrap_return(user_context: *const UserContext) -> !;
}

/// Called from asm_usertrap_entry after GPRs are saved into UserContext.
/// Interrupts are disabled (hardware cleared CRMD.IE on exception entry).
#[unsafe(no_mangle)]
pub extern "C" fn usertrap_handler() -> ! {
    // Close out the user-time accumulator before doing any kernel work
    // (trap_return will later assert it was opened here).
    trap::trap_enter();

    let era = csr::read::<{ csr::num::ERA }>();
    current::tcb().user_context().set_user_entry(era);

    let estat = csr::read::<{ csr::num::ESTAT }>();
    let ecode = (estat & csr::estat::ECODE_MASK) >> csr::estat::ECODE_SHIFT;

    match ecode {
        csr::ecode::SYS => handle_syscall(),
        csr::ecode::INT => handle_interrupt(estat),
        csr::ecode::PIL | csr::ecode::PNR => handle_memory_fault(MemAccessType::Read),
        csr::ecode::PIS | csr::ecode::PME => handle_memory_fault(MemAccessType::Write),
        csr::ecode::PIF | csr::ecode::PNX => handle_memory_fault(MemAccessType::Execute),
        csr::ecode::PPI => handle_memory_fault(MemAccessType::Read),
        csr::ecode::INE | csr::ecode::IPE => trap::illegal_inst(),
        csr::ecode::ALE => trap::memory_misaligned(),
        csr::ecode::ADE => handle_ade(),
        _ => {
            let badv = csr::read::<{ csr::num::BADV }>();
            let badi = csr::read::<{ csr::num::BADI }>();
            panic!(
                "loongarch: unhandled user trap: Ecode={:#x}, ESTAT={:#x}, \
                 ERA={:#x}, BADV={:#x}, BADI={:#x}",
                ecode, estat, era, badv, badi,
            );
        }
    }

    return_to_user();
}

fn handle_syscall() {
    let tcb = current::tcb();
    let uc = tcb.user_context();

    let args: &[usize; 7] = (&uc.gpr[4..11]).try_into().expect("slice len");
    let num = uc.gpr[11];
    let a0_pre = uc.gpr[4];

    uc.gpr[4] = trap::syscall(num, args, a0_pre);
}

fn handle_interrupt(estat: usize) {
    let is = estat & csr::estat::IS_MASK;

    if is & (1 << csr::ecfg::LINE_TIMER) != 0 {
        csr::write::<{ csr::num::TICLR }>(csr::ticlr::TIMER_ACK);
        trap::timer_interrupt();
    }

    if is & (1 << csr::ecfg::LINE_HWI0) != 0 {
        while let Some(irq) = eiointc::claim_irq() {
            trap::external_interrupt(irq);
            eiointc::complete_irq(irq);
        }
    }

    let other = is & !(1 << csr::ecfg::LINE_TIMER) & !(1 << csr::ecfg::LINE_HWI0);
    if other != 0 {
        kwarn!("loongarch: unroutable user-side interrupt lines {:#x}", other);
    }
}

fn handle_memory_fault(access: MemAccessType) {
    let badv = csr::read::<{ csr::num::BADV }>();
    trap::memory_fault(badv, access);
}

fn handle_ade() {
    let badv = csr::read::<{ csr::num::BADV }>();
    let estat = csr::read::<{ csr::num::ESTAT }>();
    let esub = (estat >> csr::estat::ESUBCODE_SHIFT) & csr::estat::ESUBCODE_MASK;
    if esub == 0 {
        trap::memory_fault(badv, MemAccessType::Execute);
    } else {
        trap::memory_fault(badv, MemAccessType::Write);
    }
}

pub fn return_to_user() -> ! {
    trap::trap_return();

    let tcb = current::tcb();
    let uc = tcb.user_context();

    csr::write::<{ csr::num::SAVE0 }>(uc as *const UserContext as usize);

    use crate::arch::arch::ArchTrait;
    uc.kernel_percpu = crate::arch::arch::Arch::get_percpu_data();

    csr::write::<{ csr::num::PGDL }>(uc.user_pgd);

    unsafe {
        core::arch::asm!("invtlb 0x00, $zero, $zero", options(nostack, preserves_flags));
    }

    csr::write::<{ csr::num::ERA  }>(uc.get_user_entry());
    csr::write::<{ csr::num::PRMD }>(csr::prmd::USERFRAME);

    unsafe { asm_usertrap_return(uc as *const UserContext) }
}
