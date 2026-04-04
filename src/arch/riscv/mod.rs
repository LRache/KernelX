mod arch;
mod cpu;
pub mod csr;
mod fdt;
mod pagetable;
mod plic;
mod sbi_driver;
mod task;

pub use context::{KernelContext, SigContext, UserContext};
pub use fdt::load_device_tree;
pub use pagetable::*;
pub use switch::kernel_switch;
pub use task::*;

use cpu::{core_count, time_frequency};

pub const PGBITS: usize = 12; // 4KB page size
pub const PGSIZE: usize = 1 << PGBITS; // 4096 bytes
pub const PGMASK: usize = PGSIZE - 1; // 0xfff
pub const TRAMPOLINE_BASE: usize = 0xffff_ffff_ffff_f000;
