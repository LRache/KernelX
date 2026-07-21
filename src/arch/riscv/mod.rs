mod arch;
mod cpu;
mod crc32;
pub mod csr;
mod fdt;
pub mod kmodule;
mod pagetable;
mod plic;
mod sbi_driver;
mod task;

pub use context::{KernelContext, SigContext, UserContext};
pub use fdt::load_device_tree;
pub use pagetable::*;
pub use switch::kernel_switch;
pub use task::*;

cfg_if::cfg_if! {
    if #[cfg(feature = "kvm")] {
        mod kvm;
        pub use kvm::{KvmPageFault, KvmRegs, KvmSRegs, VCpu};

        pub type KvmPageTable = Sv39x4PageTable;
    }
}

use cpu::{core_count, time_frequency, try_time_frequency};

pub const PGBITS: usize = 12; // 4KB page size
pub const PGSIZE: usize = 1 << PGBITS; // 4096 bytes
pub const PGMASK: usize = PGSIZE - 1; // 0xfff
pub const KERNEL_MMIO_START: usize = 0xffff_fffe_8000_0000;
pub const KERNEL_STACK_ARENA_START: usize = 0xffff_ffff_8000_0000;
pub const KERNEL_STACK_ARENA_END: usize = 0xffff_ffff_c000_0000;
pub const KERNEL_MMIO_END: usize = KERNEL_STACK_ARENA_START;
pub const USEREND: usize = 0x7fff_ffff_ffff; // 128TB user space limit
