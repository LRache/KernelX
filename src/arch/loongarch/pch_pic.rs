//! Platform Controller Hub — Programmable Interrupt Controller (PCH-PIC).
//!
//! PCH-PIC is the "south-bridge" interrupt aggregator on the LS7A / LA
//! QEMU virt machine. Devices assert their legacy INTx line to PCH-PIC,
//! which then routes each input to an EIOINTC vector via per-IRQ
//! `HTVEC(irq)` and `INT_ROUTE(irq)` register bytes.
//!
//! We take the identity (pass-through) routing policy: PCH-PIC IRQ N
//! fires EIOINTC IRQ N, so every device's FDT-declared `interrupts = <N>`
//! propagates straight through. `enable_device_interrupt_irq(N)` calls
//! `pch_pic::enable_irq(N)` followed by `eiointc::enable_irq(N)`.
//!
//! ## Register map (64 bytes of per-IRQ metadata + a handful of headers)
//!
//! | Offset       | Width | Purpose                                     |
//! |--------------|-------|---------------------------------------------|
//! | `0x00` ID    | 8     | Chip identifier + version                   |
//! | `0x20` MASK  | 8     | bit=1 → line masked (reset = all masked)    |
//! | `0x30` HTMSI | 8     | bit=1 → use MSI for that line (we force 0) |
//! | `0x40` EDGE  | 8     | bit=1 → edge-triggered                      |
//! | `0x50` CLEAR | 8     | W1C ack for edge-triggered sources          |
//! | `0x60` AUTO0 | 8     | auto-routing group 0 (leave 0)              |
//! | `0x68` AUTO1 | 8     | auto-routing group 1 (leave 0)              |
//! | `0x100+n` HTVEC | 1 per IRQ | EIOINTC vector to raise (we set = n)  |
//! | `0x200+n` ROUTE | 1 per IRQ | HT line bitmap to deliver on (we set = 1) |
//!
//! Reference: Loongson 7A1000 bridge manual §5; Linux
//! drivers/irqchip/irq-loongson-pch-pic.c.

use crate::arch;
use crate::kinfo;

/// Number of PCH-PIC inputs. The 7A1000 exposes 64 but QEMU virt uses the
/// low 32; sizing to 64 costs nothing and matches Linux.
const NR_IRQS: usize = 64;

mod reg {
    pub const MASK:   usize = 0x20;  // 8 bytes (64 bits)
    pub const HTMSI:  usize = 0x30;
    // pub const EDGE:   usize = 0x40;
    pub const CLEAR:  usize = 0x50;
    pub const HTVEC_BASE: usize = 0x100;
    pub const ROUTE_BASE: usize = 0x200;
}

/// Kernel-visible MMIO base (DMW0 mirror). Set by `init()`.
static mut BASE: usize = 0;

#[inline]
fn base() -> usize {
    // SAFETY: BASE is written once in `init()` during single-core boot and
    // never mutated afterward. Every read comes after `init()` returns.
    // Any caller who reads BASE before init() will get 0 and dereference
    // null, which trips the debug_assert below.
    unsafe { BASE }
}

#[inline]
fn write_d(off: usize, value: u64) {
    let base = base();
    debug_assert!(base != 0, "pch_pic accessed before init()");
    unsafe { arch::write_volatile((base + off) as *mut u64, value) }
}

#[inline]
fn read_d(off: usize) -> u64 {
    let base = base();
    debug_assert!(base != 0, "pch_pic accessed before init()");
    unsafe { arch::read_volatile((base + off) as *const u64) }
}

#[inline]
fn write_b(off: usize, value: u8) {
    let base = base();
    debug_assert!(base != 0, "pch_pic accessed before init()");
    unsafe { arch::write_volatile((base + off) as *mut u8, value) }
}

/// Initialize PCH-PIC.
///
/// - `mmio_pa` / `_mmio_size` come from the FDT `reg` property of the
///   `loongson,pch-pic-1.0` node (typically 0x10000000 + 0x400 on QEMU virt).
/// - All IRQs start masked; drivers unmask their lines via
///   `Arch::enable_device_interrupt_irq`.
pub fn init(mmio_pa: usize, mmio_size: usize) {
    // SAFETY: Single-core, called once from Arch::scan_device before any
    // other pch_pic::* function runs. No racing reader is possible here.
    let kaddr = arch::mmio_phys_to_kaddr(mmio_pa, mmio_size);
    unsafe { BASE = kaddr };

    // MMIO mirror via DMW0. We rely on mmio_phys_to_kaddr returning the
    // uncached view on LA — that's a postcondition of the trait.
    debug_assert!(kaddr >> 60 == 0x8, "PCH-PIC kaddr {:#x} not in DMW0", kaddr);

    // Mask everything. Linux does the same: rely on drivers to unmask
    // their specific IRQ before consuming it.
    write_d(reg::MASK, !0u64);

    // Disable MSI routing for legacy INTx use.
    write_d(reg::HTMSI, 0);

    // Clear any pending edge-triggered lines that may have latched.
    write_d(reg::CLEAR, !0u64);

    // Pass-through HTVEC: PCH-PIC IRQ n → EIOINTC vector n.
    // Pass-through ROUTE:  IRQ n → HT line 0 (bit 0). Single HT line to
    // EIOINTC input; QEMU virt has exactly one.
    for n in 0..NR_IRQS {
        write_b(reg::HTVEC_BASE + n, n as u8);
        write_b(reg::ROUTE_BASE + n, 0x01);
    }

    kinfo!(
        "loongarch: PCH-PIC initialized @ PA {:#x} / kaddr {:#x} ({} IRQs, all masked)",
        mmio_pa, kaddr, NR_IRQS,
    );
}

/// Unmask IRQ `n` on PCH-PIC. The write is bit-granular: MASK bit=0 → unmasked.
pub fn enable_irq(irq: u32) {
    debug_assert!((irq as usize) < NR_IRQS, "PCH-PIC irq {} out of range", irq);
    let bit = 1u64 << irq;
    let mut cur = read_d(reg::MASK);
    cur &= !bit;
    write_d(reg::MASK, cur);
}

/// Mask IRQ `n` on PCH-PIC.
#[allow(dead_code)]
pub fn disable_irq(irq: u32) {
    debug_assert!((irq as usize) < NR_IRQS, "PCH-PIC irq {} out of range", irq);
    let bit = 1u64 << irq;
    let mut cur = read_d(reg::MASK);
    cur |= bit;
    write_d(reg::MASK, cur);
}

/// Acknowledge an edge-triggered IRQ. For level-triggered sources (most
/// of the QEMU virt devices) this is a no-op — the source device deasserts
/// when its driver reads the status register. Kept as a hook for future
/// edge-triggered drivers.
#[allow(dead_code)]
pub fn ack_irq(irq: u32) {
    debug_assert!((irq as usize) < NR_IRQS, "PCH-PIC irq {} out of range", irq);
    write_d(reg::CLEAR, 1u64 << irq);
}
