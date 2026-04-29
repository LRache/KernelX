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

/// Write a single byte (for per-IRQ route/vec tables).
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
