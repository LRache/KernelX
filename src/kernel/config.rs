use crate::arch;

pub const USER_STACK_TOP: usize = 1 << 38; // Example user stack top address
pub const USER_STACK_PAGE_COUNT_MAX: usize = 2048; // Example user stack page count

pub const USER_BRK_BASE: usize = 0x1_0000_0000; // Base address for user brk
pub const USER_BRK_MAX: usize = 0x2_0000_0000; // Maximum reachable brk, exclusive heap end boundary

pub const USER_MAP_BASE: usize = USER_BRK_MAX; // Base address for user mappings

pub const USER_EXEC_ADDR_BASE: usize = 0x1_0000;
pub const USER_LINKER_ADDR_BASE: usize = 0x4000_0000; // Base address for the dynamic linker
pub const USER_RANDOM_ADDR_BASE: usize = 0x1000;

pub const VDSO_BASE: usize = 0x20_0000_0000; // Base address for vDSO mapping

#[cfg(debug_assertions)]
pub const UTASK_KSTACK_PAGE_COUNT: usize = 64; // Kernel stack page count for user tasks
#[cfg(not(debug_assertions))]
pub const UTASK_KSTACK_PAGE_COUNT: usize = 8; // Kernel stack page count for user tasks
pub const KTASK_KSTACK_PAGE_COUNT: usize = 32; // Kernel stack page count for kernel tasks
pub const KERNEL_HEAP_SIZE: usize = 128 * 1024 * 1024; // Kernel heap size (128 MiB)
pub const SCHEDULER_KSTACK_PAGE_COUNT: usize = 4; // Scheduler kernel stack size

cfg_if::cfg_if!(
    if #[cfg(feature = "swap-memory")] {
        pub const KERNEL_PAGE_SHRINK_WATERLEVEL_LOW : usize = 70; // LOW Threshold% for kernel page shrinker
        pub const KERNEL_PAGE_SHRINK_WATERLEVEL_HIGH: usize = 85; // HIGH Threshold% for kernel page shrinker
    }
);

pub const INODE_CACHE_SIZE: usize = 32768; // Inode cache size
pub const EXT4_INODE_PAGE_CACHE_SIZE: usize = 512; // Ext4 inode page cache size
pub const INODE_CACHE_RECLAIM_INTERVAL_MS: u64 = 500; // Background inode cache reclaim interval

pub const MAX_FD: usize = 1024; // Maximum number of file descriptors per process

pub const MAX_FILENAME_LEN: usize = 255;
pub const MAX_PATH_LEN: usize = 4096;
pub const MAX_SYMLINK_DEPTH: usize = 40;

// pub const PIPE_CAPACITY: usize = 0x20000; // Capacity of the pipe buffer
pub const PIPE_BUFFER_PAGES: usize = 16; // Number of pages allocated for pipe buffer
pub const PIPE_CAPACITY: usize = PIPE_BUFFER_PAGES * arch::PGSIZE;

/* ------ BOOT ARGS ------- */
pub const DEFAULT_BOOT_ROOT_DEVICE: &str = match option_env!("CONFIG_DEFAULT_BOOT_ROOT_DEVICE") {
    Some(v) => v,
    None => "virtio_block0",
};
pub const DEFAULT_BOOT_ROOT_FSTYPE: &str = "ext4";
pub const DEFAULT_SECOND_DEVICE: Option<&str> = option_env!("CONFIG_SECOND_DEVICE");
pub const DEFAULT_SECOND_FSTYPE: &str = match option_env!("CONFIG_SECOND_FSTYPE") {
    Some(v) => v,
    None => "ext4",
};
pub const DEFAULT_SECOND_MOUNTPOINT: &str = match option_env!("CONFIG_SECOND_MOUNTPOINT") {
    Some(v) => v,
    None => "/mnt",
};
pub const DEFAULT_INITPATH: &str = match option_env!("CONFIG_DEFAULT_INITPATH") {
    Some(v) => v,
    None => "/init",
};
pub const DEFAULT_INITCWD: &str = match option_env!("CONFIG_DEFAULT_INITCWD") {
    Some(v) => v,
    None => "/",
};
pub const DEFAULT_INITTTY: &str = match option_env!("CONFIG_DEFAULT_INITTTY") {
    Some(v) => v,
    None => "/dev/serial@10000000",
};
pub const DEFAULT_BOOTARGS: &str = match option_env!("CONFIG_DEFAULT_BOOTARGS") {
    Some(v) => v,
    None => "",
};
/* ------ BOOT ARGS ------- */
