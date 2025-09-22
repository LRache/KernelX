use crate::kernel::mm::page;
use crate::safe_page_write;

pub struct PhysPageFrame {
    page: usize,
}

impl PhysPageFrame {
    pub fn new() -> Self {
        let page = page::alloc();
        Self { page }
    }

    pub fn new_zeroed() -> Self {
        let page = page::alloc_zero();
        Self { page }
    }

    pub fn copy(&self) -> PhysPageFrame {
        let new_frame = PhysPageFrame::new();
        page::copy(self.page, new_frame.page);
        new_frame
    }

    pub fn copy_from_slice(&self, offset: usize, src: &[u8]) {
        safe_page_write!(self.page + offset, src);
    }

    pub fn get_page(&self) -> usize {
        self.page
    }
}

impl Drop for PhysPageFrame {
    fn drop(&mut self) {
        page::free(self.page);
    }
}
