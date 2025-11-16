pub const USER_STACK_TOP: usize = 1 << 38; // Example user stack top address
pub const USER_STACK_PAGE_COUNT_MAX: usize = 2048; // Example user stack page count

pub const USER_BRK_BASE: usize = 0x1_0000_0000;
pub const USER_BRK_PAGE_COUNT_MAX: usize = 16384; // Max pages for user heap (brk)

pub const USER_MAP_BASE: usize = 0x2_0000_0000; // Base address for user mappings

pub const USER_EXEC_ADDR_BASE: usize = 0x1_0000;
pub const USER_LINKER_ADDR_BASE: usize = 0x4000_0000; // Base address for the dynamic linker
pub const USER_RANDOM_ADDR_BASE: usize = 0x1000;

pub const VDSO_BASE: usize = 0x20_0000_0000; // Base address for vDSO mapping

pub const KERNEL_STACK_PAGE_COUNT: usize = 2048; // Kernel stack page count
pub const KERNEL_HEAP_SIZE: usize = 0x4000000;

pub const INODE_CACHE_SIZE: usize = 32768; // Inode cache size

pub const MAX_FD: usize = 255; // Maximum number of file descriptors per process

pub const PIPE_CAPACITY: usize = 4096; // Capacity of the pipe buffer

/* ------ BOOT ARGS ------- */
pub const DEFAULT_BOOT_ROOT_FSTYPE: &str = "ext4";
pub const DEFAULT_BOOT_ROOT: &str = "virtio_block0";
pub const DEFAULT_INITPATH: &str = "/init";
pub const DEFAULT_INITCWD: &str = "/";
/* ------ BOOT ARGS ------- */
