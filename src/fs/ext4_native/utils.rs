pub(super) const EXT4_CRC32_INIT: u32 = crate::klib::crc32c::CRC32C_INIT;

pub(super) fn crc32c(seed: u32, buf: &[u8]) -> u32 {
    crate::arch::crc32c(seed, buf)
}
