use crate::arch::KernelContext;

unsafe extern "C" {
    fn asm_kernel_switch(from: *mut KernelContext, to: *mut KernelContext);
}

pub fn kernel_switch(from: *mut KernelContext, to: *mut KernelContext) {
    unsafe {
        asm_kernel_switch(from, to);
    }
}
