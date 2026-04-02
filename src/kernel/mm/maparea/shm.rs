use alloc::boxed::Box;
use alloc::sync::Arc;

use crate::kernel::mm::{AddrSpace, MapPerm, MemAccessType};
use crate::kernel::ipc::shm::{ShmFrames, on_shm_area_drop};
use crate::klib::SpinLock;
use crate::arch::{self, PageTable, PageTableTrait};

use super::Area;

pub struct ShmArea {
    ubase: usize,
    frames: Arc<ShmFrames>,
    perm: MapPerm,
    shmid: usize,
}

impl ShmArea {
    pub fn new(ubase: usize, frames: Arc<ShmFrames>, perm: MapPerm, shmid: usize) -> Self {
        Self { ubase, frames, perm, shmid }
    }
}

impl Drop for ShmArea {
    fn drop(&mut self) {
        on_shm_area_drop(self.shmid);
    }
}

impl Area for ShmArea {
    fn translate_read(&mut self, uaddr: usize, _addrspace: &AddrSpace) -> Option<usize> {
        let page_index = (uaddr - self.ubase) / arch::PGSIZE;
        let frames = self.frames.frames.lock();
        if page_index < frames.len() {
             Some(frames[page_index].get_page())
        } else {
            None
        }
    }
    
    fn translate_write(&mut self, uaddr: usize, _addrspace: &AddrSpace) -> Option<usize> {
        self.translate_read(uaddr, _addrspace)
    }

    // fn page_frames(&mut self) -> &mut [Frame] {
    //     unimplemented!("ShmArea does not support page_frames")
    // }

    fn ubase(&self) -> usize {
        self.ubase
    }

    fn perm(&self) -> MapPerm {
        self.perm
    }

    fn page_count(&self) -> usize {
        self.frames.frames.lock().len()
    }

    fn fork(&mut self, _self_pagetable: &SpinLock<PageTable>, _new_pagetable: &mut PageTable) -> Box<dyn Area> {
        // Forked child gets its own ShmArea; increment ref_count to match.
        use crate::kernel::ipc::shm::on_shm_area_attach;
        on_shm_area_attach(self.shmid);
        Box::new(ShmArea {
            ubase: self.ubase,
            frames: self.frames.clone(),
            perm: self.perm,
            shmid: self.shmid,
        })
    }
    
    fn try_to_fix_memory_fault(&mut self, uaddr: usize, _access_type: MemAccessType, addrspace: &Arc<AddrSpace>) -> bool {
        let page_index = (uaddr - self.ubase) / arch::PGSIZE;
        let frames = self.frames.frames.lock();
        if page_index >= frames.len() {
            return false;
        }

        let frame = &frames[page_index];
        addrspace.pagetable().lock().mmap(uaddr & !arch::PGMASK, frame.get_page(), self.perm);
        true
    }
    
    fn unmap(&mut self, pagetable: &SpinLock<PageTable>) {
        let mut pt = pagetable.lock();
        let frames = self.frames.frames.lock();
        for i in 0..frames.len() {
            pt.munmap(self.ubase + i * arch::PGSIZE);
        }
    }
    
    fn type_name(&self) -> &'static str {
        "ShmArea"
    }
}
