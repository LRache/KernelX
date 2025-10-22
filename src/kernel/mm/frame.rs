use crate::kernel::mm::page;
use crate::{arch, safe_page_write};

#[derive(Debug)]
pub struct PhysPageFrame {
    page: usize,
}

impl PhysPageFrame {
    pub fn new(page: usize) -> Self {
        Self { page }
    }

    pub fn alloc() -> Self {
        Self::new(page::alloc())
    }

    pub fn alloc_zeroed() -> Self {
        Self::new(page::alloc_zero())
    }

    pub fn copy(&self) -> PhysPageFrame {
        let new_frame = PhysPageFrame::alloc();
        page::copy(self.page, new_frame.page);
        new_frame
    }

    pub fn copy_from_slice(&self, offset: usize, src: &[u8]) {
        safe_page_write!(self.page + offset, src);
    }

    pub fn slice(&self) -> &mut [u8] {
        unsafe { core::slice::from_raw_parts_mut(self.page as *mut u8, arch::PGSIZE) }
    }

    pub fn get_page(&self) -> usize {
        self.page
    }

    pub fn ptr(&self) -> *mut u8 {
        self.page as *mut u8
    }
}

impl Drop for PhysPageFrame {
    fn drop(&mut self) {
        page::free(self.page);
    }
}
