mod init;
mod vm;
mod process;
mod csr;

pub use init::init;
pub use context::{UserContext, KernelContext};
pub use switch::kernel_switch;
pub use vm::*;
pub use process::*;
pub use pagetable::PageTable;

pub const PGBITS: usize = 12; // 4KB page size
pub const PGSIZE: usize = 1 << PGBITS; // 4096 bytes
pub const PGMASK: usize = PGSIZE - 1; // 0xfff
