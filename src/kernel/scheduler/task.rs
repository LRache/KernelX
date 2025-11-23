use alloc::sync::Arc;

use crate::arch::KernelContext;
use crate::kernel::task::TCB;

pub trait Task {
    fn get_kernel_context_ptr(&self) -> *mut KernelContext;

    fn tcb(self: &Arc<Self>) -> &Arc<TCB> {
        unimplemented!()
    }

    fn kthread(self: &Arc<Self>) -> &Arc<KThread> {
        unimplemented!()
    }
}
