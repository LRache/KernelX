use crate::ffi::ext4_sblock;

pub fn get_block_size(sb: &ext4_sblock) -> u32 {
    1024u32 << u32::from_le(sb.log_block_size)
}

pub fn revision_tuple(sb: &ext4_sblock) -> (u32, u16) {
    (u32::from_le(sb.rev_level), u16::from_le(sb.minor_rev_level))
}
