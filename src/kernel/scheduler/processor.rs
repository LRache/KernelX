use alloc::sync::Arc;

use crate::arch;
use crate::kernel::scheduler::current;
use crate::kernel::scheduler::task::Task;
use crate::kernel::task::TCB;

pub struct Processor<'a> {
    idle_kernel_context: arch::KernelContext,
    task: &'a Arc<dyn Task>,
}

impl<'a> Processor<'a> {
    pub fn new(task: &'a Arc<dyn Task>) -> Self {
        Self {
            idle_kernel_context: arch::KernelContext::new_idle(),
            task,
        }
    }

    pub fn task(&self) -> &Arc<dyn Task> {
        &self.task
    }

    pub fn tcb(&self) -> &TCB {
        self.task.tcb()
    }

    pub fn switch_to_task(&mut self) {
        current::set(self);
        arch::kernel_switch(&mut self.idle_kernel_context, self.task.get_kcontext_ptr());
        current::clear();
    }

    pub fn schedule(&mut self) {
        arch::disable_interrupt();
        arch::kernel_switch(self.task.get_kcontext_ptr(), &mut self.idle_kernel_context);
    }
}
