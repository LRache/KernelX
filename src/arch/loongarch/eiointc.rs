//! Extended I/O Interrupt Controller (EIOINTC).
//!
//! EIOINTC is a per-core interrupt aggregator that sits between PCH-PIC
//! (chip-level interrupts) and the CPU core's 8 hardware IRQ lines
//! (HWI0..HWI7). It is reached exclusively through **IOCSR** space —
//! no MMIO aperture, no FDT `reg` property — so the offsets below are
//! IOCSR addresses, not physical addresses.
//!
//! ## Topology (QEMU virt, single socket)
//!
//! ```text
//!  PCH-PIC IRQ N ──[HTVEC(N)]──▶ EIOINTC IRQ N ──[IPMAP/ROUTE]──▶ CPU HWI0
//! ```
//!
//! We take the Linux-style pass-through approach: PCH-PIC IRQ N maps
//! 1:1 to EIOINTC IRQ N (via HTVEC(N) = N), and every EIOINTC IRQ
//! is routed to the same CPU HWI line (HWI0, since CONFIG_NO_SMP is
//! implied for Phase 6). The trap handler demuxes the actual IRQ by
//! reading ISR.
//!
//! ## Register layout (Loongson 3A5000 / 2K2000 / ls7a)
//!
//! | Offset             | Purpose                                         |
//! |--------------------|-------------------------------------------------|
//! | `0x420`  (MISC)    | bit48 = EXT_IOI_EN, bit49 = INT_ENCODE          |
//! | `0x14a0` (NODETYPE)| Per-node routing (single node: leave 0)         |
//! | `0x14c0` (IPMAP)   | 32 bytes; each covers 32 IRQs, picks HWI lines  |
//! | `0x1600` (ENABLE)  | 32 bytes bitmap, 1 = IRQ unmasked               |
//! | `0x1800` (ISR)     | 32 bytes bitmap, 1 = pending (read to claim)    |
//! | `0x1c00` (ROUTE)   | 256 bytes, per-IRQ CPU bitmap (single CPU: 1)   |
//!
//! See Loongson Reference Manual §11.2 for the full list.

use super::{csr, iocsr};
use crate::kinfo;

/// Total number of IRQs the EIOINTC can deliver. 256 is enough for the
/// PCH-PIC set plus MSI vectors — we don't use the top half yet.
const NR_IRQS: usize = 256;

mod reg {
    // 64-bit registers reached via iocsr_read_d / iocsr_write_d.
    pub const MISC:     usize = 0x0420;
    // pub const NODETYPE: usize = 0x14a0;  // only relevant on multi-socket

    // Range bases; a single 64-bit word holds 64 IRQs.
    pub const IPMAP_BASE:  usize = 0x14c0; // 32 bytes total, 8-byte strided
    pub const ENABLE_BASE: usize = 0x1600; // 32 bytes total
    pub const ISR_BASE:    usize = 0x1800; // 32 bytes total
    pub const ROUTE_BASE:  usize = 0x1c00; // 256 bytes (1 byte per IRQ)
}

mod misc_bits {
    /// Enable extended I/O interrupts globally.
    pub const EXT_IOI_EN:   u64 = 1 << 48;
    /// Use the extended encoding (required when EXT_IOI_EN=1).
    pub const INT_ENCODE:   u64 = 1 << 49;
}

/// All IRQs start masked so no spurious interrupt fires before drivers
/// call `enable_irq`. `init` must be called before `enable_irq` /
/// `claim_irq`; this is enforced in `Arch::scan_device`'s ordering.
pub fn init() {
    // 1. Turn EXT_IOI_EN + INT_ENCODE on. Other MISC bits stay at reset.
    let mut misc = iocsr::iocsr_read_d(reg::MISC);
    misc |= misc_bits::EXT_IOI_EN | misc_bits::INT_ENCODE;
    iocsr::iocsr_write_d(reg::MISC, misc);

    // 2. Clear every ENABLE bit (32 bytes → 4 × u64). Mask everything.
    for i in 0..4 {
        iocsr::iocsr_write_d(reg::ENABLE_BASE + i * 8, 0);
    }

    // 3. IPMAP: each byte covers 32 IRQs; the byte's value is a bitmap
    //    of HWI lines to fire. We send everything to HWI0 → byte = 0x01.
    //    The 32 bytes live in 4 × u64, each u64 = 0x0101_0101_0101_0101.
    for i in 0..4 {
        iocsr::iocsr_write_d(reg::IPMAP_BASE + i * 8, 0x0101_0101_0101_0101);
    }

    // 4. ROUTE: one byte per IRQ, bitmap of target CPU cores. Single-core
    //    so every IRQ routes to CPU0 → byte = 0x01. 256 bytes = 32 × u64.
    for i in 0..(NR_IRQS / 8) {
        iocsr::iocsr_write_d(reg::ROUTE_BASE + i * 8, 0x0101_0101_0101_0101);
    }

    // 5. Flip ECFG.LIE bit 2 (LINE_HWI0) so the CPU pays attention when
    //    EIOINTC raises HWI0. Idempotent; `Arch::enable_device_interrupt`
    //    also OR's the same bit.
    let bit = 1usize << csr::ecfg::LINE_HWI0;
    csr::xchg::<{ csr::num::ECFG }>(bit, bit);

    kinfo!("loongarch: EIOINTC initialized (256 IRQs → HWI0, all masked)");
}

/// Unmask `irq` on the EIOINTC side. Caller must also unmask at PCH-PIC
/// if the source is behind PCH-PIC (which in our topology it always is).
pub fn enable_irq(irq: u32) {
    debug_assert!((irq as usize) < NR_IRQS, "EIOINTC irq {} out of range", irq);
    let word_idx = (irq as usize) / 64;
    let bit = 1u64 << ((irq as usize) % 64);
    let off = reg::ENABLE_BASE + word_idx * 8;
    let mut cur = iocsr::iocsr_read_d(off);
    cur |= bit;
    iocsr::iocsr_write_d(off, cur);
}

/// Mask `irq` at the EIOINTC layer.
#[allow(dead_code)]
pub fn disable_irq(irq: u32) {
    debug_assert!((irq as usize) < NR_IRQS, "EIOINTC irq {} out of range", irq);
    let word_idx = (irq as usize) / 64;
    let bit = 1u64 << ((irq as usize) % 64);
    let off = reg::ENABLE_BASE + word_idx * 8;
    let mut cur = iocsr::iocsr_read_d(off);
    cur &= !bit;
    iocsr::iocsr_write_d(off, cur);
}

/// Pick the lowest-numbered pending IRQ and return it. Returns None when
/// no IRQ is pending — the trap handler breaks the claim loop on None.
///
/// Note: this does **not** acknowledge the IRQ; call `complete_irq`
/// after the driver handler has finished. If the driver fails to call
/// complete_irq, the ISR bit stays set and we'll see the same IRQ on
/// the next HWI0 trap.
pub fn claim_irq() -> Option<u32> {
    for word_idx in 0..4 {
        let pending = iocsr::iocsr_read_d(reg::ISR_BASE + word_idx * 8);
        if pending != 0 {
            let lsb = pending.trailing_zeros();
            return Some((word_idx as u32) * 64 + lsb);
        }
    }
    None
}

/// Acknowledge `irq`. ISR is W1C — writing a 1 clears the bit. The
/// PCH-PIC side is acked separately in `pch_pic::ack_irq` (edge-triggered
/// lines only); for level-triggered lines the source device deasserts
/// once its driver reads the condition register, and PCH-PIC follows.
pub fn complete_irq(irq: u32) {
    debug_assert!((irq as usize) < NR_IRQS, "EIOINTC irq {} out of range", irq);
    let word_idx = (irq as usize) / 64;
    let bit = 1u64 << ((irq as usize) % 64);
    iocsr::iocsr_write_d(reg::ISR_BASE + word_idx * 8, bit);
}
