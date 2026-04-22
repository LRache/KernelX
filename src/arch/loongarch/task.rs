//! Task support for LoongArch: kernel-context switch glue.
//!
//! The real assembly (`asm_la_kernel_switch`) will be added in Phase 4/5.
//! Until then, the Rust wrapper exists so `arch::kernel_switch` and
//! `crate::arch::loongarch::kernel_switch` re-exports type-check.

use crate::arch::KernelContext;

pub fn kernel_switch(_from: *mut KernelContext, _to: *mut KernelContext) {
    unimplemented!("loongarch: kernel_switch (Phase 4/5)");
}
