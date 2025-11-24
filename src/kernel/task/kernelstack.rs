use crate::arch;
use crate::kernel::mm;

pub struct KernelStack {
    top: usize,
    page_count: usize,
}

impl KernelStack {
    pub fn new(page_count: usize) -> Self {
        let base = mm::page::alloc_contiguous(page_count);
        let top = base + arch::PGSIZE * page_count;
        Self { top, page_count }
    }

    pub fn get_top(&self) -> usize {
        self.top
    }

    pub fn stack_overflow(&self, sp: usize) -> bool {
        let base = self.top - arch::PGSIZE * self.page_count;
        sp < self.top && sp >= base
    }
}

impl Drop for KernelStack {
    fn drop(&mut self) {
        let base = self.top - arch::PGSIZE * self.page_count;
        mm::page::free_contiguous(base, self.page_count);
    }
}

unsafe impl Send for KernelStack {}
unsafe impl Sync for KernelStack {}