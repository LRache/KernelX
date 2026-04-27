//! LoongArch I/O CSR (IOCSR) access primitives.
//!
//! The EIOINTC lives entirely in IOCSR space — it does **not** sit on the
//! MMIO bus and has no entry in the FDT `reg` of a bus node. Its registers
//! are reached with `iocsrrd.{w,d}` / `iocsrwr.{w,d}` instructions, where
//! the offset is a runtime register operand (unlike regular CSRs which
//! require an immediate).
//!
//! We keep this a small separate module rather than tacking it onto
//! `csr.rs`: the CSR module is already large, and IOCSR has a different
//! instruction family / addressing model.

/// Read a 64-bit IOCSR register.
#[inline]
pub fn iocsr_read_d(offset: usize) -> u64 {
    let v: u64;
    unsafe {
        core::arch::asm!(
            "iocsrrd.d {v}, {off}",
            v = out(reg) v,
            off = in(reg) offset,
            options(nostack, preserves_flags),
        );
    }
    v
}

/// Write a 64-bit IOCSR register.
#[inline]
pub fn iocsr_write_d(offset: usize, value: u64) {
    unsafe {
        core::arch::asm!(
            "iocsrwr.d {v}, {off}",
            v = in(reg) value,
            off = in(reg) offset,
            options(nostack, preserves_flags),
        );
    }
}

/// Read a 32-bit IOCSR register.
#[inline]
#[allow(dead_code)]
pub fn iocsr_read_w(offset: usize) -> u32 {
    let v: u32;
    unsafe {
        core::arch::asm!(
            "iocsrrd.w {v}, {off}",
            v = out(reg) v,
            off = in(reg) offset,
            options(nostack, preserves_flags),
        );
    }
    v
}

/// Write a 32-bit IOCSR register.
#[inline]
#[allow(dead_code)]
pub fn iocsr_write_w(offset: usize, value: u32) {
    unsafe {
        core::arch::asm!(
            "iocsrwr.w {v}, {off}",
            v = in(reg) value,
            off = in(reg) offset,
            options(nostack, preserves_flags),
        );
    }
}

/// Write a single byte to IOCSR (used for per-IRQ route/vec tables).
#[inline]
pub fn iocsr_write_b(offset: usize, value: u8) {
    unsafe {
        core::arch::asm!(
            "iocsrwr.b {v}, {off}",
            v = in(reg) value as u32,
            off = in(reg) offset,
            options(nostack, preserves_flags),
        );
    }
}
