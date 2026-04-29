//! LoongArch64 architecture backend.

mod arch;
mod boot;
mod context;
mod csr;
mod eiointc;
mod fdt;
mod iocsr;
mod pagetable;
mod pch_pic;
mod pci;
mod task;
mod trap;

pub use context::{KernelContext, SigContext, UserContext};
pub use pagetable::PageTable;
pub use task::kernel_switch;

/// 4 KiB base page, matching Linux's default LoongArch configuration.
pub const PGBITS: usize = 12;
pub const PGSIZE: usize = 1 << PGBITS;
pub const PGMASK: usize = PGSIZE - 1;

/// Kept numerically aligned with the RISC-V port so generic code (signal
pub const TRAMPOLINE_BASE: usize = 0xffff_ffff_ffff_f000;
