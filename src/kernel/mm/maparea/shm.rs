use alloc::boxed::Box;
use alloc::sync::Arc;
use spin::RwLock;

use crate::kernel::mm::{MapPerm, MemAccessType};
use crate::arch::{self, PageTable, PageTableTrait};
use crate::kernel::mm::maparea::area::{Area, Frame};
use crate::kernel::ipc::shm::ShmFrames;

pub struct ShmArea {
    ubase: usize,
    frames: Arc<ShmFrames>,
    perm: MapPerm,
}

impl ShmArea {
    pub fn new(ubase: usize, frames: Arc<ShmFrames>, perm: MapPerm) -> Self {
        Self {
            ubase,
            frames,
            perm,
        }
    }
}

impl Area for ShmArea {
    fn translate_read(&mut self, uaddr: usize, _pagetable: &RwLock<PageTable>) -> Option<usize> {
        let page_index = (uaddr - self.ubase) / arch::PGSIZE;
        let frames = self.frames.frames.lock();
        if page_index < frames.len() {
             Some(frames[page_index].get_page())
        } else {
            None
        }
    }
    
    fn translate_write(&mut self, uaddr: usize, _pagetable: &RwLock<PageTable>) -> Option<usize> {
        self.translate_read(uaddr, _pagetable)
    }

    fn page_frames(&mut self) -> &mut [Frame] {
        unimplemented!("ShmArea does not support page_frames")
    }

    fn ubase(&self) -> usize {
        self.ubase
    }

    fn perm(&self) -> MapPerm {
        self.perm
    }

    fn page_count(&self) -> usize {
        self.frames.frames.lock().len()
    }

    fn fork(&mut self, _self_pagetable: &RwLock<PageTable>, _fork_pagetable: &RwLock<PageTable>) -> Box<dyn Area> {
        Box::new(ShmArea {
            ubase: self.ubase,
            frames: self.frames.clone(),
            perm: self.perm,
        })
    }
    
    fn try_to_fix_memory_fault(&mut self, uaddr: usize, access_type: MemAccessType, pagetable: &RwLock<PageTable>) -> bool {
        let page_index = (uaddr - self.ubase) / arch::PGSIZE;
        let frames = self.frames.frames.lock();
        if page_index >= frames.len() {
            return false;
        }
        
        if access_type == MemAccessType::Write && !self.perm.contains(MapPerm::W) {
            return false;
        }
        if access_type == MemAccessType::Read && !self.perm.contains(MapPerm::R) {
             return false;
        }

        let frame = &frames[page_index];
        pagetable.write().mmap(uaddr & !arch::PGMASK, frame.get_page(), self.perm);
        true
    }
    
    fn unmap(&mut self, pagetable: &RwLock<PageTable>) {
        let mut pt = pagetable.write();
        let frames = self.frames.frames.lock();
        for i in 0..frames.len() {
            pt.munmap(self.ubase + i * arch::PGSIZE);
        }
    }
    
    fn type_name(&self) -> &'static str {
        "ShmArea"
    }
}
