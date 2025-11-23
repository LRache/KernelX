use alloc::vec::Vec;
use alloc::sync::Arc;

use crate::kernel::mm::PhysPageFrame;
use crate::klib::SpinLock;

pub enum ShmPageFrame {
    Allocated(PhysPageFrame),
    Unallocated,
}

pub struct ShmFrames {
    pub frames: SpinLock<Vec<PhysPageFrame>>
}

impl ShmFrames {
    pub fn new(page_count: usize) -> Self {
        let mut frames = Vec::new();
        for _ in 0..page_count {
            frames.push(PhysPageFrame::alloc());
        }
        ShmFrames {
            frames: SpinLock::new(frames)
        }
    }

    pub fn page_count(&self) -> usize {
        self.frames.lock().len()
    }
}
