use crate::arch;
use crate::kernel::mm;
use crate::kernel::config::KERNEL_STACK_PAGE_COUNT;

pub struct KernelStack {
    base: usize,
}

impl KernelStack {
    pub fn new() -> Self {
        let page = mm::page::alloc_contiguous(KERNEL_STACK_PAGE_COUNT);
        Self {
            base: page,
        }
    }

    pub fn get_top(&self) -> usize {
        self.base + arch::PGSIZE * KERNEL_STACK_PAGE_COUNT
    }
}

impl Drop for KernelStack {
    fn drop(&mut self) {
        mm::page::free_contiguous(self.base, KERNEL_STACK_PAGE_COUNT);
    }
}

unsafe impl Send for KernelStack {}
unsafe impl Sync for KernelStack {}