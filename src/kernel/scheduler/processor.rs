use alloc::sync::Arc;

use crate::kernel::scheduler::task::Task;
use crate::kernel::task::TCB;
use crate::arch;

pub struct Processor {
    hart_id: usize,
    task: *const Arc<dyn Task>,
    idle_kernel_context: arch::KernelContext,
}

impl<'a> Processor {
    pub fn new(hart_id: usize) -> Self {
        Self {
            hart_id,
            task: 0 as *const Arc<dyn Task>,
            idle_kernel_context: arch::KernelContext::new_idle(),
        }
    }

    pub fn hart_id(&self) -> usize {
        self.hart_id
    }

    pub fn has_task(&self) -> bool {
        !self.task.is_null()
    }
    pub fn task(&self) -> &'a Arc<dyn Task> {
        unsafe { &*self.task }
    }

    pub fn tcb(&self) -> &TCB {
        self.task().tcb()
    }

    pub fn switch_to_task(&mut self, task: &'a Arc<dyn Task>) {
        self.task = task;
        arch::kernel_switch(&mut self.idle_kernel_context, task.get_kcontext_ptr());
        self.task = 0 as *const Arc<dyn Task>;
    }

    pub fn schedule(&mut self) {
        arch::disable_interrupt();
        arch::kernel_switch(self.task().get_kcontext_ptr(), &mut self.idle_kernel_context);
    }
}
