use alloc::sync::Arc;

use crate::arch;
use crate::kernel::task::TCB;

pub struct Processor<'a> {
    pub idle_kernel_context: arch::KernelContext,
    pub tcb: &'a mut Arc<TCB>,
}

impl<'a> Processor<'a> {
    pub fn new(tcb: &'a mut Arc<TCB>) -> Self {
        Self {
            idle_kernel_context: arch::KernelContext::new_idle(),
            tcb,
        }
    }

    pub fn switch_to_task(&mut self){
        let kernel_context_ptr = self.tcb.get_kernel_context_ptr();
        arch::kernel_switch(&mut self.idle_kernel_context, kernel_context_ptr);
    }

    pub fn schedule(&mut self) {
        arch::disable_interrupt();
        let kernel_context_ptr = self.tcb.get_kernel_context_ptr();
        arch::kernel_switch(kernel_context_ptr, &mut self.idle_kernel_context);
    }
}
