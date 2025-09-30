pub const USER_STACK_TOP: usize = 1 << 38; // Example user stack top address
pub const USER_STACK_PAGE_COUNT_MAX: usize = 16; // Example user stack page count

pub const USER_BRK_BASE: usize = 0x80000000;
pub const USER_BRK_PAGE_COUNT_MAX: usize = 64;

pub const USER_MAP_BASE: usize = 0x1_00000000; // Base address for user mappings

pub const USER_EXEC_ADDR_BASE: usize = 0x10000;
pub const USER_LINKER_ADDR_BASE: usize = 0x40000000; // Base address for the dynamic linker
pub const USER_RANDOM_ADDR_BASE: usize = 0x1000;

pub const KERNEL_STACK_PAGE_COUNT: usize = 12; // Kernel stack page count

pub const INODE_CACHE_SIZE: usize = 512; // Inode cache size

pub const MAX_FD: usize = 255; // Maximum number of file descriptors per process

pub const PIPE_CAPACITY: usize = 4096; // Capacity of the pipe buffer
