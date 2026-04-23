//! LoongArch exception / interrupt dispatcher.
//!
//! Phase 4 scope:
//!   - **kernel** trap entry only. User-space doesn't run yet (that's Phase 5),
//!     so everything that arrives here came from PLV0. Any page fault is a
//!     kernel bug — we treat them like `panic!`.
//!   - Decode ESTAT.{Ecode,IS} and dispatch to:
//!       · `IS bit 11` / Ecode=0x00 (INT) → timer interrupt
//!       · anything else → panic with a readable message
//!   - Timer handler acks TICLR, bumps software timers, and may re-schedule.
//!
//! The asm side (`clib/src/arch/loongarch/trap/kerneltrap.S`) saves every
//! GPR that a C-ABI-safe function might clobber, calls into
//! `kerneltrap_handler`, restores them, and executes `ertn`.

use core::sync::atomic::{AtomicUsize, Ordering};

use crate::arch::loongarch::csr;
use crate::kernel::trap;
use crate::{kinfo, kwarn};

/// Count timer interrupts we've taken. Exposed for observability only —
/// the scheduler's wall-clock advancement goes through `timer_interrupt()`
/// into `kernel::event::timer` which has its own bookkeeping.
static TIMER_TICKS: AtomicUsize = AtomicUsize::new(0);

/// Read-only accessor for debug / tests.
pub fn timer_tick_count() -> usize {
    TIMER_TICKS.load(Ordering::Relaxed)
}

/// Called from asm. ra/sp/$r21 are already stashed — the handler sees a
/// stable kernel stack and can panic/log freely.
///
/// Contract with asm:
///   - interrupts globally disabled on entry (hardware clears CRMD.IE on
///     exception); we keep them off for the whole handler.
///   - ERA already holds the PC to return to; we don't adjust it (timer
///     interrupts don't skip an instruction).
#[unsafe(no_mangle)]
pub extern "C" fn kerneltrap_handler() {
    let estat = csr::read::<{ csr::num::ESTAT }>();
    let ecode = (estat & csr::estat::ECODE_MASK) >> csr::estat::ECODE_SHIFT;
    let is = estat & csr::estat::IS_MASK;
    let era = csr::read::<{ csr::num::ERA }>();

    if ecode == csr::ecode::INT {
        // Interrupt path. Multiple lines can fire together; check each bit.
        if is & (1 << csr::ecfg::LINE_TIMER) != 0 {
            // Clear the timer's pending bit BEFORE the handler runs. Otherwise
            // we re-trap immediately on `ertn`.
            csr::write::<{ csr::num::TICLR }>(csr::ticlr::TIMER_ACK);
            TIMER_TICKS.fetch_add(1, Ordering::Relaxed);
            trap::timer_interrupt();
        }

        // External IRQs (HWI0..HWI7) arrive on the same Ecode=INT path. No
        // driver consumes them yet — Phase 6 will wire a LS7A / PCH-PIC
        // dispatch here. For now just log so we notice if something fires
        // unexpectedly.
        let other = is & !(1 << csr::ecfg::LINE_TIMER);
        if other != 0 {
            kwarn!(
                "loongarch: unroutable interrupt lines {:#x} @ ERA={:#x}",
                other,
                era
            );
        }
        return;
    }

    // Synchronous exception from kernel mode. Shouldn't happen in Phase 4 —
    // there's no user code running yet — so every case is a bug.
    let badv = csr::read::<{ csr::num::BADV }>();
    let badi = csr::read::<{ csr::num::BADI }>();
    panic!(
        "loongarch: unhandled kernel trap: Ecode={:#x} ({}), \
         ESTAT={:#x}, ERA={:#x}, BADV={:#x}, BADI={:#x}",
        ecode,
        ecode_name(ecode),
        estat,
        era,
        badv,
        badi,
    );
}

/// Readable label for a given ESTAT.Ecode. Used exclusively by the panic
/// message so the caller sees "PIS" instead of "2".
fn ecode_name(ecode: usize) -> &'static str {
    match ecode {
        csr::ecode::INT   => "INT",
        csr::ecode::PIL   => "PIL (load, NR)",
        csr::ecode::PIS   => "PIS (store, no-PLV / NW)",
        csr::ecode::PIF   => "PIF (fetch, NX)",
        csr::ecode::PME   => "PME (first-write dirty)",
        csr::ecode::PNR   => "PNR (page NR)",
        csr::ecode::PNX   => "PNX (page NX)",
        csr::ecode::PPI   => "PPI (privilege)",
        csr::ecode::ADE   => "ADE (addr error)",
        csr::ecode::ALE   => "ALE (unaligned)",
        csr::ecode::BCE   => "BCE",
        csr::ecode::SYS   => "SYS (syscall)",
        csr::ecode::BRK   => "BRK (break)",
        csr::ecode::INE   => "INE (illegal inst)",
        csr::ecode::IPE   => "IPE (inst priv err)",
        csr::ecode::FPD   => "FPD (FPU disabled)",
        csr::ecode::SXD   => "SXD",
        csr::ecode::ASXD  => "ASXD",
        csr::ecode::FPE   => "FPE",
        _ => "unknown",
    }
}

/// Install `asm_kerneltrap_entry` into EENTRY and set VS=0 so every exception
/// lands at the same address. Called from `Arch::init`.
///
/// Split out as its own function mainly so the asm extern is local to this
/// file — no point in polluting `arch.rs` with it.
pub fn install_trap_entry() {
    unsafe extern "C" {
        fn asm_kerneltrap_entry() -> !;
    }

    let entry_addr = asm_kerneltrap_entry as usize;
    debug_assert!(
        entry_addr & 0xfff == 0,
        "EENTRY must be page-aligned, got {:#x}",
        entry_addr,
    );

    csr::write::<{ csr::num::EENTRY }>(entry_addr);
    // VS=0 → single entry (no vectored dispatch).
    csr::xchg::<{ csr::num::ECFG }>(0, csr::ecfg::VS_MASK);

    kinfo!(
        "loongarch: EENTRY = {:#x} (single-entry, Ecode-dispatched)",
        entry_addr
    );
}
