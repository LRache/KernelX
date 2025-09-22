pub const KERNEL_VADDR_OFFSET: usize = 0xffffffff00000000;
pub const KERNEL_PMEM_TOP: usize = 0x80000000 + 0x8000000; // 256MiB

pub const TRAMPOLINE_BASE: usize = 0_usize.wrapping_sub(4096);
