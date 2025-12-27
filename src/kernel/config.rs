pub const USER_STACK_TOP: usize = 1 << 38; // Example user stack top address
pub const USER_STACK_PAGE_COUNT_MAX: usize = 2048; // Example user stack page count

pub const USER_BRK_BASE: usize = 0x1_0000_0000; // Base address for user brk

pub const USER_MAP_BASE: usize = 0x2_0000_0000; // Base address for user mappings

pub const USER_EXEC_ADDR_BASE: usize = 0x1_0000;
pub const USER_LINKER_ADDR_BASE: usize = 0x4000_0000; // Base address for the dynamic linker
pub const USER_RANDOM_ADDR_BASE: usize = 0x1000;

pub const VDSO_BASE: usize = 0x20_0000_0000; // Base address for vDSO mapping

pub const UTASK_KSTACK_PAGE_COUNT: usize = 8; // Kernel stack page count for user tasks
pub const KTASK_KSTACK_PAGE_COUNT: usize = 16; // Kernel stack page count for kernel tasks
pub const KERNEL_HEAP_SIZE: usize = 0x4000000;
pub const KERNEL_PAGE_SHRINK_WATERLEVEL_LOW : usize = 70; // LOW Threshold% for kernel page shrinker
pub const KERNEL_PAGE_SHRINK_WATERLEVEL_HIGH: usize = 85; // HIGH Threshold% for kernel page shrinker
pub const SCHEDULER_KSTACK_PAGE_COUNT: usize = 4; // Scheduler kernel stack size

pub const INODE_CACHE_SIZE: usize = 32768; // Inode cache size

pub const MAX_FD: usize = 1024; // Maximum number of file descriptors per process

pub const PIPE_CAPACITY: usize = 0x20000; // Capacity of the pipe buffer

/* ------ BOOT ARGS ------- */
pub const DEFAULT_BOOT_ROOT_FSTYPE: &str = "ext4";
pub const DEFAULT_BOOT_ROOT: &str = "virtio_block0";
pub const DEFAULT_INITPATH: &str = "/init";
pub const DEFAULT_INITCWD: &str = "/";
pub const DEFAULT_INITTTY: &str = "/dev/serial@10000000";
/* ------ BOOT ARGS ------- */
