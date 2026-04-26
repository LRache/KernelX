use alloc::sync::Arc;
use alloc::vec::Vec;

use crate::arch;
use crate::arch::{KvmPageTable, PageTableTrait};
use crate::kernel::mm::{MapPerm, MemAccessType, PhysPageFrame};
use crate::klib::SpinLock;

pub enum SharedFrame {
    Unallocated,
    Allocated(PhysPageFrame),
}

#[derive(Clone)]
pub struct SharedFrames {
    frames: Arc<SpinLock<Vec<SharedFrame>>>,
    base: usize,
    start: usize,
    len: usize,
}

impl SharedFrames {
    pub fn new(base: usize, page_count: usize) -> Self {
        let frames = (0..page_count).map(|_| SharedFrame::Unallocated).collect();
        Self {
            frames: Arc::new(SpinLock::new(frames, "kvm::SharedFrames::frames")),
            base,
            start: 0,
            len: page_count,
        }
    }

    pub fn len(&self) -> usize {
        self.len
    }

    fn page_index(&self, page_index: usize) -> usize {
        self.start + page_index
    }

    pub fn split_at(&self, split_index: usize) -> (Self, Self) {
        debug_assert!(split_index > 0 && split_index < self.len, "split index out of range");

        (
            Self {
                frames: self.frames.clone(),
                base: self.base,
                start: self.start,
                len: split_index,
            },
            Self {
                frames: self.frames.clone(),
                base: self.base + split_index * arch::PGSIZE,
                start: self.start + split_index,
                len: self.len - split_index,
            },
        )
    }

    pub fn get_page(&self, page_index: usize) -> Option<usize> {
        let frames = self.frames.lock();
        let frame = frames.get(self.page_index(page_index))?;
        match frame {
            SharedFrame::Allocated(frame) => Some(frame.get_page()),
            SharedFrame::Unallocated => None,
        }
    }

    pub fn translate(&self, page_index: usize) -> usize {
        let mut frames = self.frames.lock();
        let frame = frames.get_mut(self.page_index(page_index)).unwrap();
        match frame {
            SharedFrame::Allocated(frame) => frame.get_page(),
            SharedFrame::Unallocated => {
                let new_frame = PhysPageFrame::alloc_zeroed();
                let kpage = new_frame.get_page();
                *frame = SharedFrame::Allocated(new_frame);
                kpage
            }
        }
    }

    pub fn try_to_fix_memory_fault(&self, page_index: usize) -> usize {
        let mut frames = self.frames.lock();
        let frame = frames.get_mut(self.page_index(page_index)).unwrap();
        match &*frame {
            SharedFrame::Allocated(frame) => frame.get_page(),
            SharedFrame::Unallocated => {
                let new_frame = PhysPageFrame::alloc_zeroed();
                let kpage = new_frame.get_page();
                *frame = SharedFrame::Allocated(new_frame);
                kpage
            }
        }
    }
}

pub struct VMMapArea {
    gbase: usize,
    perm: MapPerm,
    frames: SharedFrames,
}

impl VMMapArea {
    pub fn new(gbase: usize, perm: MapPerm, page_count: usize) -> Self {
        debug_assert!(gbase % arch::PGSIZE == 0, "gbase should be page-aligned");

        let frames = SharedFrames::new(gbase, page_count);
        Self { gbase, perm, frames }
    }

    pub fn shared_frames(&self) -> SharedFrames {
        self.frames.clone()
    }

    pub fn gbase(&self) -> usize {
        self.gbase
    }

    pub fn page_count(&self) -> usize {
        self.frames.len()
    }

    pub fn size(&self) -> usize {
        self.page_count() * arch::PGSIZE
    }

    pub fn try_to_fix_memory_fault(
        &mut self,
        gaddr: usize,
        access_type: MemAccessType,
        pagetable: &SpinLock<KvmPageTable>,
    ) -> Option<usize> {
        if gaddr < self.gbase || !access_type.match_perm(self.perm) {
            return None;
        }

        let page_index = (gaddr - self.gbase) / arch::PGSIZE;
        if page_index >= self.frames.len() {
            return None;
        }

        let gpage = self.gbase + page_index * arch::PGSIZE;
        let kpage = self.frames.try_to_fix_memory_fault(page_index);

        let mut pagetable = pagetable.lock();
        if pagetable.is_mapped(gpage) {
            pagetable.mmap_replace(gpage, kpage, self.perm);
        } else {
            pagetable.mmap(gpage, kpage, self.perm);
        }

        Some(kpage + gaddr % arch::PGSIZE)
    }
}
