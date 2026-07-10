use bitflags::bitflags;

use crate::kernel::syscall::UserStruct;

bitflags! {
    #[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
    pub struct StatfsFlags: u64 {
        const ST_RDONLY = 0x1;
    }
}

#[repr(C)]
#[derive(Debug, Clone, Copy, Default, UserStruct)]
pub struct Statfs {
    pub f_type: u64,
    pub f_bsize: u64,
    pub f_blocks: u64,
    pub f_bfree: u64,
    pub f_bavail: u64,
    pub f_files: u64,
    pub f_ffree: u64,
    pub f_fsid: u64,
    pub f_namelen: u64,
    pub f_frsize: u64,
    pub f_flag: u64,
    pub f_spare: [u64; 4],
}
