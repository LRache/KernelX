use alloc::sync::Arc;

use crate::kernel::task::Tid;
use crate::kernel::task::TCB;

pub struct KThread {
    pub tid: Tid,
    kernel_context: KernelContext,
    pub kernel_stack: KernelStack,
}


