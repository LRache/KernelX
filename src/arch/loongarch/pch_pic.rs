use crate::arch;
use crate::kinfo;

/// 7A1000 exposes 64 inputs; sizing to 64 matches Linux.
const NR_IRQS: usize = 64;

mod reg {
    pub const MASK:        usize = 0x20;
    pub const HTMSI_EN:    usize = 0x40;
    pub const CLEAR:       usize = 0x80;
    pub const ROUTE_BASE:  usize = 0x100;
    pub const HTVEC_BASE:  usize = 0x200;
}

static mut BASE: usize = 0;

#[inline]
fn base() -> usize {
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

pub fn init(mmio_pa: usize, mmio_size: usize) {
    let kaddr = arch::mmio_phys_to_kaddr(mmio_pa, mmio_size);
    unsafe { BASE = kaddr };

    debug_assert!(kaddr >> 60 == 0x8, "PCH-PIC kaddr {:#x} not in DMW0", kaddr);

    write_d(reg::MASK, !0u64);
    write_d(reg::HTMSI_EN, 0);
    write_d(reg::CLEAR, !0u64);

    for n in 0..NR_IRQS {
        write_b(reg::HTVEC_BASE + n, n as u8);
        write_b(reg::ROUTE_BASE + n, 0x01);
    }

    kinfo!(
        "loongarch: PCH-PIC initialized @ PA {:#x} / kaddr {:#x} ({} IRQs, all masked)",
        mmio_pa, kaddr, NR_IRQS,
    );
}

pub fn enable_irq(irq: u32) {
    debug_assert!((irq as usize) < NR_IRQS, "PCH-PIC irq {} out of range", irq);
    let bit = 1u64 << irq;
    let mut cur = read_d(reg::MASK);
    cur &= !bit;
    write_d(reg::MASK, cur);
}

#[allow(dead_code)]
pub fn disable_irq(irq: u32) {
    debug_assert!((irq as usize) < NR_IRQS, "PCH-PIC irq {} out of range", irq);
    let bit = 1u64 << irq;
    let mut cur = read_d(reg::MASK);
    cur |= bit;
    write_d(reg::MASK, cur);
}

#[allow(dead_code)]
pub fn ack_irq(irq: u32) {
    debug_assert!((irq as usize) < NR_IRQS, "PCH-PIC irq {} out of range", irq);
    write_d(reg::CLEAR, 1u64 << irq);
}
