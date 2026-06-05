use alloc::sync::Arc;
use core::ptr::NonNull;

#[cfg(feature = "deadlock-detect")]
use alloc::collections::vec_deque::VecDeque;

use crate::arch;
use crate::kernel::scheduler::task::Task;
use crate::kernel::task::TCB;

pub struct Processor {
    hart_id: usize,
    task: Option<NonNull<Arc<dyn Task>>>,
    idle_kernel_context: arch::KernelContext,

    #[cfg(feature = "deadlock-detect")]
    locked_spin: VecDeque<&'static str>,
}

impl<'a> Processor {
    pub fn new(hart_id: usize) -> Self {
        Self {
            hart_id,
            task: None,
            idle_kernel_context: arch::KernelContext::new_idle(),
            #[cfg(feature = "deadlock-detect")]
            locked_spin: VecDeque::new(),
            // irq_spinlock_count: 0,
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
        task.resume_system_time();
        self.task = Some(NonNull::from(task));
        arch::kernel_switch(&mut self.idle_kernel_context, task.kcontext_ptr());
        self.task = None;
    }

    pub fn schedule(&mut self) {
        #[cfg(feature = "deadlock-detect")]
        if !self.locked_spin.is_empty() {
            use crate::println;

            println!("Spinlocks held by processor {} when trying to schedule:", self.hart_id);
            let mut i = 0;
            while let Some(name) = self.locked_spin.pop_back() {
                println!("[{}] {}", i, name);
                i += 1;
            }
            panic!(
                "Processor {} is trying to schedule while holding spinlocks.",
                self.hart_id
            );
        }
        arch::disable_interrupt();
        self.task().pause_system_time();
        arch::kernel_switch(self.task().kcontext_ptr(), &mut self.idle_kernel_context);
        self.task().resume_system_time();
    }

    #[cfg(feature = "deadlock-detect")]
    pub fn acquire_spinlock(&mut self, name: &'static str) {
        self.locked_spin.push_back(name);
    }

    #[cfg(feature = "deadlock-detect")]
    pub fn release_spinlock(&mut self, name: &'static str) {
        if let Some(pos) = self.locked_spin.iter().rposition(|&n| n == name) {
            self.locked_spin.remove(pos);
        } else {
            panic!(
                "Processor {} is releasing spinlock '{}' which it does not hold!",
                self.hart_id, name
            );
        }
    }
}
