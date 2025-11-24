mod arch;
mod csr;
mod fdt;
mod pagetable;
mod process;
mod sbi_driver;

pub use context::{KernelContext, SigContext, UserContext};
pub use fdt::{load_device_tree, time_frequency};
pub use pagetable::PageTable;
pub use process::*;
pub use switch::kernel_switch;

pub const PGBITS: usize = 12; // 4KB page size
pub const PGSIZE: usize = 1 << PGBITS; // 4096 bytes
pub const PGMASK: usize = PGSIZE - 1; // 0xfff
pub const TRAMPOLINE_BASE: usize = 0xffff_ffff_ffff_f000;
