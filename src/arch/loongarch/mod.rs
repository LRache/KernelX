//! LoongArch64 architecture backend.

mod arch;
mod boot;
mod context;
mod csr;
pub(crate) mod eiointc;
mod fdt;
mod iocsr;
mod pagetable;
pub(crate) mod pch_pic;
mod pci;
mod task;
pub(crate) mod trap;

pub use context::{KernelContext, SigContext, UserContext};
pub use pagetable::PageTable;
pub use task::kernel_switch;

/// 4 KiB base page, matching Linux's default LoongArch configuration.
pub const PGBITS: usize = 12;
pub const PGSIZE: usize = 1 << PGBITS;
pub const PGMASK: usize = PGSIZE - 1;

/// Kept numerically aligned with the RISC-V port so generic code (signal
pub const TRAMPOLINE_BASE: usize = 0xffff_ffff_ffff_f000;
/// User address space ends below kernel DMW. 48-bit VA → upper half is kernel.
pub const USEREND: usize = 0x7fff_ffff_ffff;
