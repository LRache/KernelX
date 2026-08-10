use crate::kernel::scheduler::current;
use crate::klib;

const CRC32C_POLY_WITH_TOP_BIT: u64 = 0x1_05ec_76f1;
const CRC32C_POLY_INV_MOD_X64: u64 = 0x4869_ec38_dea7_13f1;

pub(super) fn crc32c(seed: u32, buf: &[u8]) -> u32 {
    if current::has_processor() && current::processor().arch_data().zbc_supported() {
        crc32c_zbc(seed, buf)
    } else {
        klib::crc::crc32c_update_generic(seed, buf)
    }
}

fn crc32c_zbc(seed: u32, buf: &[u8]) -> u32 {
    let mut crc = seed;
    let mut offset = 0;
    while offset + 8 <= buf.len() {
        let block = u64::from_le_bytes([
            buf[offset],
            buf[offset + 1],
            buf[offset + 2],
            buf[offset + 3],
            buf[offset + 4],
            buf[offset + 5],
            buf[offset + 6],
            buf[offset + 7],
        ]);
        crc = crc32c_u64(crc, block);
        offset += 8;
    }

    crate::klib::crc::crc32c_update_generic(crc, &buf[offset..])
}

#[inline(always)]
fn crc32c_u64(crc: u32, block: u64) -> u32 {
    let dividend = block ^ crc as u64;
    // SAFETY: The caller selects this path only when the current CPU reports
    // support for the Zbc carry-less multiplication instructions.
    let quotient = unsafe { clmul(dividend, CRC32C_POLY_INV_MOD_X64) };
    // SAFETY: The same Zbc capability check covers `clmulh`.
    unsafe { clmulh(quotient, CRC32C_POLY_WITH_TOP_BIT) as u32 }
}

#[inline(always)]
unsafe fn clmul(lhs: u64, rhs: u64) -> u64 {
    let value: u64;
    unsafe {
        core::arch::asm!(
            ".insn r 0x33, 0x1, 0x05, {value}, {lhs}, {rhs}",
            value = lateout(reg) value,
            lhs = in(reg) lhs,
            rhs = in(reg) rhs,
            options(nostack, nomem),
        );
    }
    value
}

#[inline(always)]
unsafe fn clmulh(lhs: u64, rhs: u64) -> u64 {
    let value: u64;
    unsafe {
        core::arch::asm!(
            ".insn r 0x33, 0x3, 0x05, {value}, {lhs}, {rhs}",
            value = lateout(reg) value,
            lhs = in(reg) lhs,
            rhs = in(reg) rhs,
            options(nostack, nomem),
        );
    }
    value
}
