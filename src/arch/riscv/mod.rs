mod vm;
mod process;
mod csr;
mod arch;

pub use context::{UserContext, KernelContext, SigContext};
pub use switch::kernel_switch;
pub use vm::*;
pub use process::*;
pub use pagetable::PageTable;

pub const PGBITS: usize = 12; // 4KB page size
pub const PGSIZE: usize = 1 << PGBITS; // 4096 bytes
pub const PGMASK: usize = PGSIZE - 1; // 0xfff
pub const KADDR_OFFSET: usize = 0xffff_ffc0_0000_0000; // Kernel virtual address offset

unsafe extern "C" {
    fn asm_kerneltrap_entry() -> !;
}
