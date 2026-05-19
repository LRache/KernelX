use super::{csr, iocsr};
use crate::kinfo;
use core::sync::atomic::{AtomicUsize, Ordering};

const NR_IRQS: usize = 256;
static PARENT_LINE: AtomicUsize = AtomicUsize::new(csr::ecfg::LINE_HWI0);

mod reg {
    pub const MISC: usize = 0x0420;
    pub const IPMAP_BASE: usize = 0x14c0;
    pub const ENABLE_BASE: usize = 0x1600;
    pub const ISR_BASE: usize = 0x1800;
    pub const ROUTE_BASE: usize = 0x1c00;
}

mod misc_bits {
    pub const EXT_IOI_EN: u64 = 1 << 48;
}

/// All IRQs start masked; drivers call `enable_irq` explicitly.
pub fn init(parent_line: usize) {
    let parent_line = if (csr::ecfg::LINE_HWI0..csr::ecfg::LINE_HWI0 + csr::ecfg::HWI_COUNT).contains(&parent_line) {
        parent_line
    } else {
        csr::ecfg::LINE_HWI0
    };
    PARENT_LINE.store(parent_line, Ordering::Relaxed);

    let mut misc = iocsr::iocsr_read_d(reg::MISC);
    misc |= misc_bits::EXT_IOI_EN;
    iocsr::iocsr_write_d(reg::MISC, misc);

    // Mask every IRQ.
    for i in 0..4 {
        iocsr::iocsr_write_d(reg::ENABLE_BASE + i * 8, 0);
    }

    let hwi_bit = 1u64 << (parent_line - csr::ecfg::LINE_HWI0);
    let ipmap = hwi_bit
        | (hwi_bit << 8)
        | (hwi_bit << 16)
        | (hwi_bit << 24)
        | (hwi_bit << 32)
        | (hwi_bit << 40)
        | (hwi_bit << 48)
        | (hwi_bit << 56);

    // IPMAP: each byte is a HWI-line bitmap for 32 IRQs.
    for i in 0..4 {
        iocsr::iocsr_write_d(reg::IPMAP_BASE + i * 8, ipmap);
    }

    // ROUTE: per-IRQ CPU bitmap. Single-core → 0x01 for every byte.
    for i in 0..(NR_IRQS / 8) {
        iocsr::iocsr_write_d(reg::ROUTE_BASE + i * 8, 0x0101_0101_0101_0101);
    }

    // ECFG.LIE bit so the CPU responds when EIOINTC raises its parent HWI.
    // Idempotent with `Arch::enable_device_interrupt`.
    let bit = 1usize << parent_line;
    csr::xchg::<{ csr::num::ECFG }>(bit, bit);

    kinfo!(
        "loongarch: EIOINTC initialized (256 IRQs -> HWI{}, all masked)",
        parent_line - csr::ecfg::LINE_HWI0
    );
}

pub fn parent_line() -> usize {
    PARENT_LINE.load(Ordering::Relaxed)
}

/// Unmask `irq` on EIOINTC. Caller must also unmask at PCH-PIC.
pub fn enable_irq(irq: u32) {
    debug_assert!((irq as usize) < NR_IRQS, "EIOINTC irq {} out of range", irq);
    let word_idx = (irq as usize) / 64;
    let bit = 1u64 << ((irq as usize) % 64);
    let off = reg::ENABLE_BASE + word_idx * 8;
    let mut cur = iocsr::iocsr_read_d(off);
    cur |= bit;
    iocsr::iocsr_write_d(off, cur);
}

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

pub fn isr0() -> u64 {
    iocsr::iocsr_read_d(reg::ISR_BASE)
}

pub fn complete_irq(irq: u32) {
    debug_assert!((irq as usize) < NR_IRQS, "EIOINTC irq {} out of range", irq);
    let word_idx = (irq as usize) / 64;
    let bit = 1u64 << ((irq as usize) % 64);
    iocsr::iocsr_write_d(reg::ISR_BASE + word_idx * 8, bit);
}

#[allow(dead_code)]
pub fn dump_irq(irq: u32) {
    let word_idx = (irq as usize) / 64;
    let en = iocsr::iocsr_read_d(reg::ENABLE_BASE + word_idx * 8);
    let isr = iocsr::iocsr_read_d(reg::ISR_BASE + word_idx * 8);
    // IPMAP: 1 byte per 32-IRQ bucket. word_idx*2 + (irq/32 within word)
    let ipmap_idx = (irq as usize) / 32;
    let ipmap_d_idx = ipmap_idx / 8;
    let ipmap_byte_in_d = ipmap_idx % 8;
    let ipmap_d = iocsr::iocsr_read_d(reg::IPMAP_BASE + ipmap_d_idx * 8);
    let ipmap_byte = (ipmap_d >> (ipmap_byte_in_d * 8)) & 0xff;
    // ROUTE: 1 byte per IRQ, packed 8 per 64-bit word.
    let route_d_idx = (irq as usize) / 8;
    let route_byte_in_d = (irq as usize) % 8;
    let route_d = iocsr::iocsr_read_d(reg::ROUTE_BASE + route_d_idx * 8);
    let route_byte = (route_d >> (route_byte_in_d * 8)) & 0xff;
    let misc = iocsr::iocsr_read_d(reg::MISC);
    crate::kinfo!(
        "eiointc dump irq={}: EN[{}]={:#x} (bit{}={}) ISR[{}]={:#x} IPMAP[irq>>5={}]={:#x} ROUTE[irq]={:#x} MISC={:#x}",
        irq,
        word_idx,
        en,
        irq % 64,
        (en >> (irq % 64)) & 1,
        word_idx,
        isr,
        ipmap_idx,
        ipmap_byte,
        route_byte,
        misc,
    );
}
