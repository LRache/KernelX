use crate::arch;
use crate::kernel::mm::{self, MapPerm};

use super::super::PGSIZE;

pub struct KernelStack<const MAPPED_PAGE_COUNT: usize> {
    top: usize,
}

impl<const MAPPED_PAGE_COUNT: usize> KernelStack<MAPPED_PAGE_COUNT> {
    pub fn new() -> Self {
        let page_count = MAPPED_PAGE_COUNT
            .checked_add(1)
            .expect("Kernel stack page count overflow");
        assert!(
            page_count >= 2 && page_count.is_power_of_two(),
            "Kernel stack allocation must contain 2^n pages including its guard page"
        );

        let base = mm::page::alloc_contiguous(page_count);
        let top = base + PGSIZE * page_count;
        // SAFETY: The allocated guard page belongs exclusively to this stack
        // and remains allocated until it is remapped in Drop.
        unsafe { arch::unmap_kernel_addr(base, PGSIZE) };
        Self { top }
    }

    pub fn get_top(&self) -> usize {
        self.top
    }

    pub fn check_stack_overflow(&self, addr: usize) -> bool {
        let base = self.top - PGSIZE * (MAPPED_PAGE_COUNT + 1);
        addr >= base && addr < base + PGSIZE
    }
}

impl<const MAPPED_PAGE_COUNT: usize> Drop for KernelStack<MAPPED_PAGE_COUNT> {
    fn drop(&mut self) {
        let page_count = MAPPED_PAGE_COUNT + 1;
        let base = self.top - PGSIZE * page_count;
        arch::map_kernel_addr(base, arch::kaddr_to_paddr(base), PGSIZE, MapPerm::RW);
        mm::page::free_contiguous(base, page_count);
    }
}
