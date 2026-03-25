use core::ptr::NonNull;
use alloc::sync::Arc;

use crate::kernel::scheduler::task::Task;
use crate::kernel::task::TCB;
use crate::arch;

pub struct Processor {
    hart_id: usize,
    task: Option<NonNull<Arc<dyn Task>>>,
    idle_kernel_context: arch::KernelContext,
}

impl<'a> Processor {
    pub fn new(hart_id: usize) -> Self {
        Self {
            hart_id,
            task: None,
            idle_kernel_context: arch::KernelContext::new_idle(),
        }
    }

    pub fn hart_id(&self) -> usize {
        self.hart_id
    }

    pub fn has_task(&self) -> bool {
        self.task.is_some()
    }
    
    pub fn task(&self) -> &'a Arc<dyn Task> {
        let p = if cfg!(debug_assertions) {
            self.task.unwrap()
        } else {
            unsafe { self.task.unwrap_unchecked() }
        };
        unsafe { p.as_ref() }
    }

    pub fn tcb(&self) -> &TCB {
        self.task().tcb()
    }

    pub fn switch_to_task(&mut self, task: &'a Arc<dyn Task>) {
        self.task = Some(NonNull::from(task));
        arch::kernel_switch(&mut self.idle_kernel_context, task.get_kcontext_ptr());
        self.task = None;
    }

    pub fn schedule(&mut self) {
        arch::disable_interrupt();
        arch::kernel_switch(self.task().get_kcontext_ptr(), &mut self.idle_kernel_context);
    }
}
