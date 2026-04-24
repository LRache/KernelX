//! Kernel-context switch glue.
//!
//! The real asm lives in `clib/src/arch/loongarch/entry/switch.S`. This
//! file just wraps the extern so `Arch::kernel_switch` has a safe Rust
//! callable.

use crate::arch::KernelContext;

unsafe extern "C" {
    fn asm_kernel_switch(from: *mut KernelContext, to: *mut KernelContext);
}

pub fn kernel_switch(from: *mut KernelContext, to: *mut KernelContext) {
    unsafe {
        asm_kernel_switch(from, to);
    }
}
