use alloc::sync::Arc;
use core::ptr::NonNull;

#[cfg(feature = "spinlock-check")]
use alloc::collections::vec_deque::VecDeque;

use crate::arch;
use crate::kernel::scheduler::task::Task;
use crate::kernel::task::TCB;

#[cfg(feature = "spinlock-check")]
struct HeldSpinLock {
    ptr: usize,
    name: &'static str,
}

pub struct Processor {
    hart_id: usize,
    kernel_trap_stack_top: usize,
    task: Option<NonNull<Arc<dyn Task>>>,
    idle_kernel_context: arch::KernelContext,
    arch_data: arch::ArchPerCpuData,

    #[cfg(feature = "spinlock-check")]
    locked_spin: VecDeque<HeldSpinLock>,
}

impl<'a> Processor {
    pub fn new(hart_id: usize) -> Self {
        Self {
            hart_id,
            kernel_trap_stack_top: 0,
            task: None,
            idle_kernel_context: arch::KernelContext::new_idle(),
            arch_data: arch::ArchPerCpuData::new(),
            #[cfg(feature = "spinlock-check")]
            locked_spin: VecDeque::new(),
            // irq_spinlock_count: 0,
        }
    }

    pub fn hart_id(&self) -> usize {
        self.hart_id
    }

    pub fn kernel_trap_stack_top(&self) -> usize {
        self.kernel_trap_stack_top
    }

    pub fn set_kernel_trap_stack_top(&mut self, stack_top: usize) {
        self.kernel_trap_stack_top = stack_top;
    }

    pub fn arch_data(&self) -> &arch::ArchPerCpuData {
        &self.arch_data
    }

    pub fn arch_data_mut(&mut self) -> &mut arch::ArchPerCpuData {
        &mut self.arch_data
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

    pub fn switch_from_idle(&mut self, task: &'a Arc<dyn Task>) {
        self.task = Some(NonNull::from(task));
        arch::kernel_switch(&mut self.idle_kernel_context, task.kcontext_ptr());
        self.task = None;
    }

    pub fn switch_to_idle(&mut self) {
        #[cfg(feature = "spinlock-check")]
        if !self.locked_spin.is_empty() {
            use crate::println;

            println!("Spinlocks held by processor {} when trying to schedule:", self.hart_id);
            let mut i = 0;
            while let Some(lock) = self.locked_spin.pop_back() {
                println!("[{}] {} ({:#x})", i, lock.name, lock.ptr);
                i += 1;
            }
            panic!(
                "Processor {} is trying to schedule while holding spinlocks.",
                self.hart_id
            );
        }
        arch::kernel_switch(self.task().kcontext_ptr(), &mut self.idle_kernel_context);
    }

    #[cfg(feature = "spinlock-check")]
    pub fn assert_not_holding_spinlock(&self, ptr: usize, name: &'static str) {
        if self.locked_spin.iter().any(|lock| lock.ptr == ptr) {
            panic!(
                "Processor {} is trying to recursively acquire spinlock '{}' ({:#x}).",
                self.hart_id, name, ptr
            );
        }
    }

    #[cfg(feature = "spinlock-check")]
    pub fn acquire_spinlock(&mut self, ptr: usize, name: &'static str) {
        self.assert_not_holding_spinlock(ptr, name);
        self.locked_spin.push_back(HeldSpinLock { ptr, name });
    }

    #[cfg(feature = "spinlock-check")]
    pub fn release_spinlock(&mut self, ptr: usize, name: &'static str) {
        if let Some(pos) = self.locked_spin.iter().rposition(|lock| lock.ptr == ptr) {
            self.locked_spin.remove(pos);
        } else {
            panic!(
                "Processor {} is releasing spinlock '{}' ({:#x}) which it does not hold!",
                self.hart_id, name, ptr
            );
        }
    }
}
