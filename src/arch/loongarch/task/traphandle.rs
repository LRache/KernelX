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

    // Save FPU/LSX state if either FPE or SXE was enabled for this task.
    let euen = csr::read::<{ csr::num::EUEN }>();
    if euen & 0x3 != 0 {
        save_fpu_state();
    }

    let era = csr::read::<{ csr::num::ERA }>();
    current::tcb().user_context().set_user_entry(era);

    let estat = csr::read::<{ csr::num::ESTAT }>();
    let ecode = (estat & csr::estat::ECODE_MASK) >> csr::estat::ECODE_SHIFT;

    match ecode {
        csr::ecode::SYS => handle_syscall(),
        csr::ecode::INT => handle_interrupt(estat),
        csr::ecode::PIL | csr::ecode::PNR => handle_memory_fault(MemAccessType::Read),
        csr::ecode::PIS => handle_memory_fault(MemAccessType::Write),
        csr::ecode::PME => handle_page_modify(),
        csr::ecode::PIF | csr::ecode::PNX => handle_memory_fault(MemAccessType::Execute),
        csr::ecode::PPI => handle_memory_fault(MemAccessType::Read),
        csr::ecode::INE | csr::ecode::IPE => trap::illegal_inst(),
        csr::ecode::ALE => trap::memory_misaligned(),
        csr::ecode::ADE => handle_ade(),
        csr::ecode::FPD => handle_fpu_disabled(),
        csr::ecode::SXD => handle_lsx_disabled(),
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

/// PME (Page Modify Exception): the page is valid and mapped but D=0.
/// On LoongArch, D=0 serves as the write-protect mechanism (there is no
/// separate "writable" hardware bit). PME fires in two cases:
///   1. CoW page: fork removed W from perm → mmap set D=0. The write must
///      trigger a copy-on-write via the full memory_fault(Write) path which
///      will allocate a new page and mmap_replace with D=1.
///   2. Swap tracking: take_access_dirty_bit cleared D for eviction scoring.
///      The page is still writable (W in area perm), so memory_fault(Write)
///      will notice it's already Allocated and just re-map with D=1.
/// In both cases, delegating to memory_fault(Write) is correct.
fn handle_page_modify() {
    let badv = csr::read::<{ csr::num::BADV }>();
    trap::memory_fault(badv, MemAccessType::Write);
}

/// FPD (Floating-Point Disabled): user hit a float/double instruction while
/// EUEN.FPE=0.  Set the bit so the faulting instruction retries successfully.
fn handle_fpu_disabled() {
    csr::write::<{ csr::num::EUEN }>(csr::read::<{ csr::num::EUEN }>() | 0x1);
    current::tcb().user_context().fpregs_dirty = true;
}

/// SXD (LSX Disabled): user hit a 128-bit SIMD instruction while EUEN.SXE=0.
/// Enable both FPE (bit 0) and SXE (bit 1) — LSX requires FPU enabled too.
fn handle_lsx_disabled() {
    csr::write::<{ csr::num::EUEN }>(csr::read::<{ csr::num::EUEN }>() | 0x3);
    current::tcb().user_context().fpregs_dirty = true;
}

/// Save all 32 vector registers (128-bit LSX) + FCC + FCSR into UserContext.
fn save_fpu_state() {
    let uc = current::tcb().user_context();
    unsafe {
        // Enable LSX so we can use vst (it may only have FPE set, not SXE)
        csr::write::<{ csr::num::EUEN }>(csr::read::<{ csr::num::EUEN }>() | 0x3);
        // Save $vr0..$vr31 (128-bit each) into uc.fpregs[0..32] (u128 array)
        let base = uc.fpregs.as_mut_ptr() as usize;
        core::arch::asm!(
            "vst $vr0,  {base}, 0*16",
            "vst $vr1,  {base}, 1*16",
            "vst $vr2,  {base}, 2*16",
            "vst $vr3,  {base}, 3*16",
            "vst $vr4,  {base}, 4*16",
            "vst $vr5,  {base}, 5*16",
            "vst $vr6,  {base}, 6*16",
            "vst $vr7,  {base}, 7*16",
            "vst $vr8,  {base}, 8*16",
            "vst $vr9,  {base}, 9*16",
            "vst $vr10, {base}, 10*16",
            "vst $vr11, {base}, 11*16",
            "vst $vr12, {base}, 12*16",
            "vst $vr13, {base}, 13*16",
            "vst $vr14, {base}, 14*16",
            "vst $vr15, {base}, 15*16",
            "vst $vr16, {base}, 16*16",
            "vst $vr17, {base}, 17*16",
            "vst $vr18, {base}, 18*16",
            "vst $vr19, {base}, 19*16",
            "vst $vr20, {base}, 20*16",
            "vst $vr21, {base}, 21*16",
            "vst $vr22, {base}, 22*16",
            "vst $vr23, {base}, 23*16",
            "vst $vr24, {base}, 24*16",
            "vst $vr25, {base}, 25*16",
            "vst $vr26, {base}, 26*16",
            "vst $vr27, {base}, 27*16",
            "vst $vr28, {base}, 28*16",
            "vst $vr29, {base}, 29*16",
            "vst $vr30, {base}, 30*16",
            "vst $vr31, {base}, 31*16",
            base = in(reg) base,
            options(nostack),
        );
        // Save FCSR0
        let fcsr_val: u32;
        core::arch::asm!("movfcsr2gr {tmp}, $fcsr0", tmp = out(reg) fcsr_val, options(nostack));
        uc.fcsr = fcsr_val as u64;
        // Save FCC (8 condition flags)
        let mut fcc_val: u64 = 0;
        let fcc_ptr = &mut fcc_val as *mut u64 as *mut u8;
        let (t0, t1, t2, t3, t4, t5, t6, t7): (u32, u32, u32, u32, u32, u32, u32, u32);
        core::arch::asm!("movcf2gr {t}, $fcc0", t = out(reg) t0, options(nostack));
        core::arch::asm!("movcf2gr {t}, $fcc1", t = out(reg) t1, options(nostack));
        core::arch::asm!("movcf2gr {t}, $fcc2", t = out(reg) t2, options(nostack));
        core::arch::asm!("movcf2gr {t}, $fcc3", t = out(reg) t3, options(nostack));
        core::arch::asm!("movcf2gr {t}, $fcc4", t = out(reg) t4, options(nostack));
        core::arch::asm!("movcf2gr {t}, $fcc5", t = out(reg) t5, options(nostack));
        core::arch::asm!("movcf2gr {t}, $fcc6", t = out(reg) t6, options(nostack));
        core::arch::asm!("movcf2gr {t}, $fcc7", t = out(reg) t7, options(nostack));
        fcc_ptr.write(t0 as u8);
        fcc_ptr.add(1).write(t1 as u8);
        fcc_ptr.add(2).write(t2 as u8);
        fcc_ptr.add(3).write(t3 as u8);
        fcc_ptr.add(4).write(t4 as u8);
        fcc_ptr.add(5).write(t5 as u8);
        fcc_ptr.add(6).write(t6 as u8);
        fcc_ptr.add(7).write(t7 as u8);
        uc.fcc = fcc_val;
    }
    // Disable FPU+LSX for kernel execution — kernel doesn't use FP/SIMD.
    csr::write::<{ csr::num::EUEN }>(csr::read::<{ csr::num::EUEN }>() & !0x3);
}

/// Restore all 32 vector registers (128-bit LSX) + FCC + FCSR from UserContext.
fn restore_fpu_state() {
    let uc = current::tcb().user_context();
    unsafe {
        // Enable FPU+LSX before restore
        csr::write::<{ csr::num::EUEN }>(csr::read::<{ csr::num::EUEN }>() | 0x3);
        let base = uc.fpregs.as_ptr() as usize;
        core::arch::asm!(
            "vld $vr0,  {base}, 0*16",
            "vld $vr1,  {base}, 1*16",
            "vld $vr2,  {base}, 2*16",
            "vld $vr3,  {base}, 3*16",
            "vld $vr4,  {base}, 4*16",
            "vld $vr5,  {base}, 5*16",
            "vld $vr6,  {base}, 6*16",
            "vld $vr7,  {base}, 7*16",
            "vld $vr8,  {base}, 8*16",
            "vld $vr9,  {base}, 9*16",
            "vld $vr10, {base}, 10*16",
            "vld $vr11, {base}, 11*16",
            "vld $vr12, {base}, 12*16",
            "vld $vr13, {base}, 13*16",
            "vld $vr14, {base}, 14*16",
            "vld $vr15, {base}, 15*16",
            "vld $vr16, {base}, 16*16",
            "vld $vr17, {base}, 17*16",
            "vld $vr18, {base}, 18*16",
            "vld $vr19, {base}, 19*16",
            "vld $vr20, {base}, 20*16",
            "vld $vr21, {base}, 21*16",
            "vld $vr22, {base}, 22*16",
            "vld $vr23, {base}, 23*16",
            "vld $vr24, {base}, 24*16",
            "vld $vr25, {base}, 25*16",
            "vld $vr26, {base}, 26*16",
            "vld $vr27, {base}, 27*16",
            "vld $vr28, {base}, 28*16",
            "vld $vr29, {base}, 29*16",
            "vld $vr30, {base}, 30*16",
            "vld $vr31, {base}, 31*16",
            base = in(reg) base,
            options(nostack),
        );
        // Restore FCSR0
        let fcsr_val = uc.fcsr as u32;
        core::arch::asm!("movgr2fcsr $fcsr0, {tmp}", tmp = in(reg) fcsr_val, options(nostack));
        // Restore FCC
        let fcc_val = uc.fcc;
        let fcc_ptr = &fcc_val as *const u64 as *const u8;
        core::arch::asm!("movgr2cf $fcc0, {tmp}", tmp = in(reg) fcc_ptr.read() as u32, options(nostack));
        core::arch::asm!("movgr2cf $fcc1, {tmp}", tmp = in(reg) fcc_ptr.add(1).read() as u32, options(nostack));
        core::arch::asm!("movgr2cf $fcc2, {tmp}", tmp = in(reg) fcc_ptr.add(2).read() as u32, options(nostack));
        core::arch::asm!("movgr2cf $fcc3, {tmp}", tmp = in(reg) fcc_ptr.add(3).read() as u32, options(nostack));
        core::arch::asm!("movgr2cf $fcc4, {tmp}", tmp = in(reg) fcc_ptr.add(4).read() as u32, options(nostack));
        core::arch::asm!("movgr2cf $fcc5, {tmp}", tmp = in(reg) fcc_ptr.add(5).read() as u32, options(nostack));
        core::arch::asm!("movgr2cf $fcc6, {tmp}", tmp = in(reg) fcc_ptr.add(6).read() as u32, options(nostack));
        core::arch::asm!("movgr2cf $fcc7, {tmp}", tmp = in(reg) fcc_ptr.add(7).read() as u32, options(nostack));
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
        core::arch::asm!(
            "dbar 0",
            "invtlb 0x00, $zero, $zero",
            "dbar 0",
            "ibar 0",
            options(nostack, preserves_flags)
        );
    }

    csr::write::<{ csr::num::ERA  }>(uc.get_user_entry());
    csr::write::<{ csr::num::PRMD }>(csr::prmd::USERFRAME);

    // Restore FPU state if this task has ever used it.
    if uc.fpregs_dirty {
        csr::write::<{ csr::num::EUEN }>(csr::read::<{ csr::num::EUEN }>() | 0x1);
        restore_fpu_state();
    }

    unsafe { asm_usertrap_return(uc as *const UserContext) }
}
