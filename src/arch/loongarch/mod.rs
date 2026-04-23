//! LoongArch64 architecture backend — **skeleton for Phase 1**.
//!
//! Only type / trait wiring is in place. Most runtime functions panic with
//! `unimplemented!()`; they will be filled in starting in Phase 2 (entry,
//! console, DMW setup), Phase 3 (page tables) and Phase 4 (traps/timer).
//!
//! Keep the module tree mirroring `src/arch/riscv/` so code review and
//! future diffs stay straightforward.

mod arch;
mod boot;
mod context;
mod csr;
mod pagetable;
mod task;
mod trap;

pub use context::{KernelContext, SigContext, UserContext};
pub use pagetable::PageTable;
pub use task::kernel_switch;

/// 4KB base page size, matching Linux's default LoongArch configuration.
pub const PGBITS: usize = 12;
pub const PGSIZE: usize = 1 << PGBITS;
pub const PGMASK: usize = PGSIZE - 1;

/// Kept at the same numeric value as the RISC-V port so that generic code
/// (signal trampolines, vDSO remap, etc.) doesn't need any arch conditionals.
/// On LoongArch this lands inside the architecturally-unreachable XUVRANGE+1,
/// so it is effectively "reserved for kernel use" as far as user space is
/// concerned.
pub const TRAMPOLINE_BASE: usize = 0xffff_ffff_ffff_f000;
