use crate::arch;
use crate::kernel::mm::{self, MapPerm};

pub struct KernelStack {
    top: usize,
    page_count: usize,
}

impl KernelStack {
    pub fn new(page_count: usize) -> Self {
        let base = mm::page::alloc_contiguous(page_count + 1);
        let top = base + arch::PGSIZE * (page_count + 1);
        unsafe { arch::unmap_kernel_addr(base, arch::PGSIZE) };
        Self { top, page_count }
    }

    pub fn get_top(&self) -> usize {
        self.top
    }

    pub fn check_stack_overflow(&self, sp: usize) -> bool {
        let base = self.top - arch::PGSIZE * (self.page_count + 1);
        sp < self.top && sp >= base
    }
}

impl Drop for KernelStack {
    fn drop(&mut self) {
        let base = self.top - arch::PGSIZE * (self.page_count + 1);
        arch::map_kernel_addr(base, arch::kaddr_to_paddr(base), arch::PGSIZE, MapPerm::RW);
        mm::page::free_contiguous(base, self.page_count + 1);
    }
}

unsafe impl Send for KernelStack {}
unsafe impl Sync for KernelStack {}