#[repr(C)]
pub struct FileStat {
    pub st_dev: u64,     // 包含文件的设备 ID
    pub st_ino: u64, // 索引节点号
    pub st_mode: u32,    // 文件类型和模式
    pub st_nlink: u32,   // 硬链接数
    pub st_uid: u32,     // 所有者的用户 ID
    pub st_gid: u32,     // 所有者的组 ID
    pub st_rdev: u64,    // 设备 ID(如果是特殊文件)
    __pad: u64,
    pub st_size: i64,    // 总大小, 以字节为单位
    pub st_blksize: i32, // 文件系统 I/O 的块大小
    __pad2: i32,
    pub st_blocks: u64,     // 分配的 512B 块数
    pub st_atime_sec: i64,  // 上次访问时间
    pub st_atime_nsec: i64, // 上次访问时间(纳秒精度)
    pub st_mtime_sec: i64,  // 上次修改时间
    pub st_mtime_nsec: i64, // 上次修改时间(纳秒精度)
    pub st_ctime_sec: i64,  // 上次状态变化的时间
    pub st_ctime_nsec: i64, // 上次状态变化的时间(纳秒精度)
    __unused: [u32; 2],
}

impl FileStat {
    pub fn new() -> Self {
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
