use alloc::vec::Vec;

use crate::kernel::mm::PhysPageFrame;
use crate::klib::SpinLock;

pub struct ShmFrames {
    pub frames: SpinLock<Vec<PhysPageFrame>>
}

impl ShmFrames {
    pub fn new(page_count: usize) -> Self {
        let frames = (0..page_count)
            .map(|_| PhysPageFrame::alloc_zeroed())
            .collect::<Vec<_>>();
        Self {
            frames: SpinLock::new(frames, "ShmFrames::frames")
        }
    }

    pub fn page_count(&self) -> usize {
        self.frames.lock().len()
    }
}
