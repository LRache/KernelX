use crate::arch::UserContextTrait;
use crate::arch::loongarch::csr::ecode::Ecode;
use crate::arch::loongarch::{UserContext, csr, eiointc, pch_pic};
use crate::kernel::mm::MemAccessType;
use crate::kernel::scheduler::current;
use crate::kernel::trap;
use crate::kwarn;

unsafe extern "C" {
    fn asm_usertrap_return(user_context: *const UserContext) -> !;
    fn asm_save_lsx(fpregs: *mut u128, fcc: *mut u64, fcsr: *mut u64);
    fn asm_restore_lsx(fpregs: *const u128, fcc: *const u64, fcsr: *const u64);
}

#[unsafe(no_mangle)]
pub extern "C" fn usertrap_handler() -> ! {
    // User execution has stopped. Kernel paths do not directly use user
    // virtual addresses, and the return path invalidates the local TLB before
    // the address space can be used again.
    current::addrspace().deactivate_cpu(current::hart_id());

    trap::trap_enter();

    // Save FPU/LSX state if either FPE or SXE was enabled for this task.
    let euen = csr::read::<{ csr::num::EUEN }>();
    if euen & 0x3 != 0 {
        save_fpu_state();
    }

    let era = csr::read::<{ csr::num::ERA }>();
    current::tcb().user_context().set_user_entry(era);

    let estat = csr::read::<{ csr::num::ESTAT }>();
    let raw_ecode = (estat & csr::estat::ECODE_MASK) >> csr::estat::ECODE_SHIFT;
    let ecode = Ecode::try_from(raw_ecode).ok();

    match ecode {
        Some(Ecode::Sys) => handle_syscall(),
        Some(Ecode::Int) => {
            let other = handle_interrupt(estat);
            if other != 0 {
                kwarn!("loongarch: unroutable user-side interrupt lines {:#x}", other);
            }
        }
        Some(Ecode::Pil) | Some(Ecode::Pnr) => handle_memory_fault(MemAccessType::Read),
        Some(Ecode::Pis) => handle_memory_fault(MemAccessType::Write),
        Some(Ecode::Pme) => handle_page_modify(),
        Some(Ecode::Pif) | Some(Ecode::Pnx) => handle_memory_fault(MemAccessType::Execute),
        Some(Ecode::Ppi) => handle_memory_fault(MemAccessType::Read),
        Some(Ecode::Ine) | Some(Ecode::Ipe) => trap::illegal_inst(),
        Some(Ecode::Ale) => trap::memory_misaligned(),
        Some(Ecode::Ade) => handle_ade(),
        Some(Ecode::Fpd) => handle_fpu_disabled(),
        Some(Ecode::Sxd) => handle_lsx_disabled(),
        _ => {
            let badv = csr::read::<{ csr::num::BADV }>();
            let badi = csr::read::<{ csr::num::BADI }>();
            panic!(
                "loongarch: unhandled user trap: Ecode={:#x}, ESTAT={:#x}, \
                 ERA={:#x}, BADV={:#x}, BADI={:#x}",
                raw_ecode, estat, era, badv, badi,
            );
        }
    }

    return_to_user();
}

#[unsafe(no_mangle)]
pub extern "C" fn kerneltrap_handler() {
    let estat = csr::read::<{ csr::num::ESTAT }>();
    let raw_ecode = (estat & csr::estat::ECODE_MASK) >> csr::estat::ECODE_SHIFT;
    let ecode = Ecode::try_from(raw_ecode).ok();
    let era = csr::read::<{ csr::num::ERA }>();

    if ecode == Some(Ecode::Int) {
        let other = handle_interrupt(estat);
        if other != 0 {
            kwarn!("loongarch: unroutable interrupt lines {:#x} @ ERA={:#x}", other, era);
        }
        return;
    }

    // Synchronous exception from kernel mode is always a bug.
    let badv = csr::read::<{ csr::num::BADV }>();
    let badi = csr::read::<{ csr::num::BADI }>();
    panic!(
        "loongarch: unhandled kernel trap: Ecode={:#x} ({}), \
         ESTAT={:#x}, ERA={:#x}, BADV={:#x}, BADI={:#x}",
        raw_ecode,
        ecode.map_or("unknown", Ecode::to_str),
        estat,
        era,
        badv,
        badi,
    );
}

fn handle_syscall() {
    let tcb = current::tcb();
    let uc = tcb.user_context();

    let args: &[usize; 7] = (&uc.gpr[4..11]).try_into().expect("slice len");
    let num = uc.gpr[11];
    let a0_pre = uc.gpr[4];

    uc.gpr[4] = trap::syscall(num, args, a0_pre);
}

fn handle_interrupt(estat: usize) -> usize {
    let is = estat & csr::estat::IS_MASK;

    if is & (1 << csr::ecfg::LINE_TIMER) != 0 {
        csr::write::<{ csr::num::TICLR }>(csr::ticlr::TIMER_ACK);
        trap::timer_interrupt();
    }

    let device_line = eiointc::parent_line();
    if is & (1 << device_line) != 0 {
        while let Some(irq) = eiointc::claim_irq() {
            eiointc::complete_irq(irq);
            trap::external_interrupt(irq);
            if pch_pic::contains_irq(irq) {
                pch_pic::ack_irq(irq);
            }
        }
    }

    is & !(1 << csr::ecfg::LINE_TIMER) & !(1 << device_line)
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

fn handle_page_modify() {
    let badv = csr::read::<{ csr::num::BADV }>();
    trap::memory_fault(badv, MemAccessType::Write);
}

fn handle_fpu_disabled() {
    csr::write::<{ csr::num::EUEN }>(csr::read::<{ csr::num::EUEN }>() | 0x1);
    current::tcb().user_context().fpregs_dirty = true;
}

fn handle_lsx_disabled() {
    csr::write::<{ csr::num::EUEN }>(csr::read::<{ csr::num::EUEN }>() | 0x3);
    current::tcb().user_context().fpregs_dirty = true;
}

/// Save all 32 vector registers (128-bit LSX) + FCC + FCSR into UserContext.
fn save_fpu_state() {
    let uc = current::tcb().user_context();
    // Enable LSX so the helper can access vector/FPU state.
    csr::write::<{ csr::num::EUEN }>(csr::read::<{ csr::num::EUEN }>() | 0x3);
    unsafe {
        asm_save_lsx(
            uc.fpregs.as_mut_ptr(),
            core::ptr::addr_of_mut!(uc.fcc),
            core::ptr::addr_of_mut!(uc.fcsr),
        );
    }
    // Disable FPU+LSX for kernel execution — kernel doesn't use FP/SIMD.
    csr::write::<{ csr::num::EUEN }>(csr::read::<{ csr::num::EUEN }>() & !0x3);
}

/// Restore all 32 vector registers (128-bit LSX) + FCC + FCSR from UserContext.
fn restore_fpu_state() {
    let uc = current::tcb().user_context();
    // Enable FPU+LSX before restoring their register files.
    csr::write::<{ csr::num::EUEN }>(csr::read::<{ csr::num::EUEN }>() | 0x3);
    unsafe {
        asm_restore_lsx(
            uc.fpregs.as_ptr(),
            core::ptr::addr_of!(uc.fcc),
            core::ptr::addr_of!(uc.fcsr),
        );
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

    // Publish this CPU under the page-table lock before the local invalidation
    // makes the user address space usable again.
    current::addrspace().activate_cpu(current::hart_id());
    crate::arch::flush_tlb_all();

    csr::write::<{ csr::num::ERA }>(uc.get_user_entry());
    csr::write::<{ csr::num::PRMD }>(csr::prmd::USERFRAME);

    if uc.fpregs_dirty {
        restore_fpu_state();
    } else {
        csr::write::<{ csr::num::EUEN }>(csr::read::<{ csr::num::EUEN }>() & !0x3);
    }

    unsafe { asm_usertrap_return(uc as *const UserContext) }
}
