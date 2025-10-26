mod pagetable;
mod sbi_driver;
mod process;
mod fdt;
mod csr;
mod arch;

pub use context::{UserContext, KernelContext, SigContext};
pub use switch::kernel_switch;
pub use process::*;
pub use pagetable::PageTable;
pub use fdt::{load_device_tree, time_frequency};

pub const PGBITS: usize = 12; // 4KB page size
pub const PGSIZE: usize = 1 << PGBITS; // 4096 bytes
pub const PGMASK: usize = PGSIZE - 1; // 0xfff
