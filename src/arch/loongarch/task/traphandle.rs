//! User-mode trap dispatch for LoongArch.
//!
//! Companion to `clib/src/arch/loongarch/trap/usertrap.S`. The asm side saves
//! the interrupted PLV3 task's GPRs into its `UserContext`, swaps in the
//! kernel stack, and calls `usertrap_handler()` below. We decode ESTAT,
//! route to the right kernel hook, and eventually fall through to
//! `return_to_user()` — which in turn trampolines back into asm to `ertn`.
//!
//! Design mirrors `src/arch/riscv/task/traphandle.rs`. Key LoongArch-specific
//! bits:
//!   - Syscall ABI: num in $a7 (gpr[11]), args in $a0..$a6 (gpr[4..11]),
//!     return in $a0. Hardware does NOT advance ERA past `syscall` — we
//!     do `user_entry += 4` manually.
//!   - Page faults: Ecode 0x01..0x07 cover read/write/fetch/modify/PLV
//!     issues. All routed to `trap::memory_fault` with a best-guess
//!     access type.
//!   - No FPU save/restore yet — Phase 9.

use crate::arch::UserContextTrait;
use crate::arch::loongarch::UserContext;
use crate::arch::loongarch::csr;
use crate::kernel::mm::MemAccessType;
use crate::kernel::scheduler::current;
use crate::kernel::trap;
use crate::kwarn;

unsafe extern "C" {
    fn asm_usertrap_return(user_context: *const UserContext) -> !;
}

/// Called from asm_usertrap_entry after GPRs are saved into the current
/// task's UserContext. Interrupts are globally disabled (CPU cleared CRMD.IE
/// on exception entry). We handle the trap, then return_to_user().
#[unsafe(no_mangle)]
pub extern "C" fn usertrap_handler() -> ! {
    // Cache ERA before anything else — the syscall path needs to advance it,
    // and any inner code running may clobber the CSR if it nests (shouldn't,
    // but defense in depth).
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
        csr::ecode::ALE | csr::ecode::ADE => trap::memory_misaligned(),
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

/// Dispatch a `syscall 0` from user space.
///
/// LoongArch LP64D ABI: number in $a7 (gpr[11]), args in $a0..$a6
/// (gpr[4..11]), return in $a0 (gpr[4]). The `syscall` instruction
/// does NOT auto-advance ERA, so we bump user_entry by 4 before returning.
fn handle_syscall() {
    let tcb = current::tcb();
    let uc = tcb.user_context();

    // gpr[4..11] is the 7-arg slice a0..a6; kernel::trap::syscall takes
    // exactly `&[usize; 7]`. We also pass the original a0 for EINTR restart.
    let args: &[usize; 7] = (&uc.gpr[4..11]).try_into().expect("slice len");
    let num = uc.gpr[11];
    let a0_pre = uc.gpr[4];

    let ret = trap::syscall(num, args, a0_pre);

    uc.gpr[4] = ret;
    // Advance past the `syscall 0` instruction (4 bytes). Using the trait
    // method so the convention lives in one place (context.rs).
    uc.skip_syscall_instruction();
}

/// Hardware interrupt arrived while user code was running. Reuse the same
/// logic as the kernel-trap path (ack timer, log stray lines). Separate
/// function so usertrap_handler stays readable.
fn handle_interrupt(estat: usize) {
    let is = estat & csr::estat::IS_MASK;

    if is & (1 << csr::ecfg::LINE_TIMER) != 0 {
        csr::write::<{ csr::num::TICLR }>(csr::ticlr::TIMER_ACK);
        trap::timer_interrupt();
    }

    let other = is & !(1 << csr::ecfg::LINE_TIMER);
    if other != 0 {
        kwarn!(
            "loongarch: unroutable user-side interrupt lines {:#x}",
            other
        );
    }
}

fn handle_memory_fault(access: MemAccessType) {
    let badv = csr::read::<{ csr::num::BADV }>();
    trap::memory_fault(badv, access);
}

/// Prepare CSRs for `ertn` back to PLV3 and jump into the asm return stub.
///
/// The asm side does all the GPR restore plus the final `ertn`; we only
/// touch things the Rust layer cares about.
pub fn return_to_user() -> ! {
    trap::trap_return();

    let tcb = current::tcb();
    let uc = tcb.user_context();

    // Tell the next usertrap the UserContext address to save into.
    // asm_usertrap_entry will csrrd $t0, SAVE0.
    csr::write::<{ csr::num::SAVE0 }>(uc as *const UserContext as usize);

    // Install the user's page-table root in PGDL. HPTW will consult it on
    // the next TLB miss (after we ertn).
    //
    // PGDL takes a physical address, page-aligned. `set_addrspace()` stashed
    // that value on the UserContext when the task was created.
    csr::write::<{ csr::num::PGDL }>(uc.user_pgd);

    // Flush the TLB unconditionally — cheap and correct. Phase 9 will move
    // to ASID-per-process + targeted invalidation.
    unsafe {
        core::arch::asm!("invtlb 0x00, $zero, $zero", options(nostack, preserves_flags));
    }

    // ERA = user PC; PRMD = {PLV=3, PIE=1} so `ertn` restores CRMD.IE=1.
    csr::write::<{ csr::num::ERA  }>(uc.get_user_entry());
    csr::write::<{ csr::num::PRMD }>(csr::prmd::USERFRAME);

    // Jump into asm to restore the GPRs and `ertn`.
    unsafe { asm_usertrap_return(uc as *const UserContext) }
}
