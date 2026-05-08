use crate::kernel::syscall::UserStruct;

#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct FileStat {
    pub st_dev: u64,
    pub st_ino: u64,
    pub st_mode: u32,
    pub st_nlink: u32,
    pub st_uid: u32,
    pub st_gid: u32,
    pub st_rdev: u64,
    __pad: u64,
    pub st_size: i64,
    pub st_blksize: i32,
    __pad2: i32,
    pub st_blocks: u64,
    pub st_atime_sec: i64,
    pub st_atime_nsec: i64,
    pub st_mtime_sec: i64,
    pub st_mtime_nsec: i64,
    pub st_ctime_sec: i64,
    pub st_ctime_nsec: i64,
    __unused: [u32; 2],
}

impl FileStat {
    pub fn empty() -> Self {
        FileStat {
            st_dev: 0,
            st_ino: 0,
            st_mode: 0,
            st_nlink: 0,
            st_uid: 0,
            st_gid: 0,
            st_rdev: 0,
            __pad: 0,
            st_size: 0,
            st_blksize: 4096,
            __pad2: 0,
            st_blocks: 0,
            st_atime_sec: 0,
            st_atime_nsec: 0,
            st_mtime_sec: 0,
            st_mtime_nsec: 0,
            st_ctime_sec: 0,
            st_ctime_nsec: 0,
            __unused: [0; 2],
        }
    }
}

impl Default for FileStat {
    fn default() -> Self {
        Self::empty()
    }
}

impl UserStruct for FileStat {}

/// `struct statx_timestamp` (include/uapi/linux/stat.h).
#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct StatxTimestamp {
    pub tv_sec: i64,
    pub tv_nsec: u32,
    __reserved: i32,
}

/// `struct statx` (256 B).
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct Statx {
    pub stx_mask: u32,
    pub stx_blksize: u32,
    pub stx_attributes: u64,
    pub stx_nlink: u32,
    pub stx_uid: u32,
    pub stx_gid: u32,
    pub stx_mode: u16,
    __spare0: [u16; 1],
    pub stx_ino: u64,
    pub stx_size: u64,
    pub stx_blocks: u64,
    pub stx_attributes_mask: u64,
    pub stx_atime: StatxTimestamp,
    pub stx_btime: StatxTimestamp,
    pub stx_ctime: StatxTimestamp,
    pub stx_mtime: StatxTimestamp,
    pub stx_rdev_major: u32,
    pub stx_rdev_minor: u32,
    pub stx_dev_major: u32,
    pub stx_dev_minor: u32,
    pub stx_mnt_id: u64,
    pub stx_dio_mem_align: u32,
    pub stx_dio_offset_align: u32,
    __spare3: [u64; 12],
}

impl Default for Statx {
    fn default() -> Self {
        // SAFETY: all fields are plain integers or zero-able.
        unsafe { core::mem::zeroed() }
    }
}

impl UserStruct for Statx {}

/// Commonly requested statx mask bits.
pub mod statx_mask {
    pub const STATX_TYPE: u32 = 0x0001;
    pub const STATX_MODE: u32 = 0x0002;
    pub const STATX_NLINK: u32 = 0x0004;
    pub const STATX_UID: u32 = 0x0008;
    pub const STATX_GID: u32 = 0x0010;
    pub const STATX_ATIME: u32 = 0x0020;
    pub const STATX_MTIME: u32 = 0x0040;
    pub const STATX_CTIME: u32 = 0x0080;
    pub const STATX_INO: u32 = 0x0100;
    pub const STATX_SIZE: u32 = 0x0200;
    pub const STATX_BLOCKS: u32 = 0x0400;
    pub const STATX_BASIC_STATS: u32 = 0x07ff;
}

impl From<FileStat> for Statx {
    fn from(s: FileStat) -> Self {
        let mut sx = Statx::default();
        sx.stx_mask = statx_mask::STATX_BASIC_STATS;
        sx.stx_blksize = s.st_blksize as u32;
        sx.stx_nlink = s.st_nlink;
        sx.stx_uid = s.st_uid;
        sx.stx_gid = s.st_gid;
        sx.stx_mode = s.st_mode as u16;
        sx.stx_ino = s.st_ino;
        sx.stx_size = s.st_size as u64;
        sx.stx_blocks = s.st_blocks;
        sx.stx_atime = StatxTimestamp {
            tv_sec: s.st_atime_sec,
            tv_nsec: s.st_atime_nsec as u32,
            ..Default::default()
        };
        sx.stx_mtime = StatxTimestamp {
            tv_sec: s.st_mtime_sec,
            tv_nsec: s.st_mtime_nsec as u32,
            ..Default::default()
        };
        sx.stx_ctime = StatxTimestamp {
            tv_sec: s.st_ctime_sec,
            tv_nsec: s.st_ctime_nsec as u32,
            ..Default::default()
        };
        sx.stx_dev_major = (s.st_dev >> 32) as u32;
        sx.stx_dev_minor = s.st_dev as u32;
        sx.stx_rdev_major = (s.st_rdev >> 32) as u32;
        sx.stx_rdev_minor = s.st_rdev as u32;
        sx
    }
}
