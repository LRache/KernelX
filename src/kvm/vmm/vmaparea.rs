use alloc::boxed::Box;
use alloc::sync::Arc;
use alloc::vec::Vec;

use crate::arch;
use crate::arch::{PageTable, PageTableTrait};
use crate::kernel::mm::maparea::{Area, SwappableNoFileFrame};
use crate::kernel::mm::{AddrSpace, MapPerm, MemAccessType};
use crate::klib::SpinLock;

pub enum SharedFrame {
    Unallocated,
    Allocated(SwappableNoFileFrame),
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
            SharedFrame::Allocated(frame) => frame.get_page(),
            SharedFrame::Unallocated => None,
        }
    }

    pub fn translate(&self, page_index: usize, addrspace: &AddrSpace) -> usize {
        let mut frames = self.frames.lock();
        let frame = frames.get_mut(self.page_index(page_index)).unwrap();
        match frame {
            SharedFrame::Allocated(frame) => frame.get_page_swap_in(),
            SharedFrame::Unallocated => {
                let addr = self.base + page_index * arch::PGSIZE;
                let (new_frame, kpage) = SwappableNoFileFrame::alloc_zeroed(addr, addrspace);
                *frame = SharedFrame::Allocated(new_frame);
                kpage
            }
        }
    }

    pub fn try_to_fix_memory_fault(&self, page_index: usize, perm: MapPerm, addrspace: &AddrSpace) -> usize {
        let mut frames = self.frames.lock();
        let frame = frames.get_mut(self.page_index(page_index)).unwrap();
        match &*frame {
            SharedFrame::Allocated(frame) => {
                let kpage = frame.get_page_swap_in();
                #[cfg(feature = "swap-memory")]
                addrspace
                    .pagetable()
                    .lock()
                    .mmap(self.base + page_index * arch::PGSIZE, kpage, perm);
                kpage
            }
            SharedFrame::Unallocated => {
                let addr = self.base + page_index * arch::PGSIZE;
                let (new_frame, kpage) = SwappableNoFileFrame::alloc_zeroed(addr, addrspace);
                addrspace
                    .pagetable()
                    .lock()
                    .mmap(self.base + page_index * arch::PGSIZE, kpage, perm);
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
}

impl Area for VMMapArea {
    fn try_to_fix_memory_fault(
        &mut self,
        gaddr: usize,
        _access_type: MemAccessType,
        addrspace: &AddrSpace,
    ) -> Option<usize> {
        debug_assert!(gaddr >= self.gbase);

        let page_index = (gaddr - self.gbase) / arch::PGSIZE;
        if page_index >= self.frames.len() {
            return None;
        }

        Some(self.frames.try_to_fix_memory_fault(page_index, self.perm(), addrspace) + gaddr % arch::PGSIZE)
    }

    fn perm(&self) -> MapPerm {
        MapPerm::R | MapPerm::W | MapPerm::X
    }

    fn translate_read(&mut self, _: usize, _: &AddrSpace) -> Option<usize> {
        unreachable!("VMMapArea should not be translated directly")
    }

    fn translate_write(&mut self, _: usize, _: &AddrSpace) -> Option<usize> {
        unreachable!("VMMapArea should not be translated directly")
    }

    fn set_perm(&mut self, _perm: MapPerm, _pagetable: &SpinLock<PageTable>) {
        unreachable!("VMMapArea permission should not be changed")
    }

    fn fork(&mut self, _: &SpinLock<PageTable>, _: &mut PageTable) -> Box<dyn Area> {
        unreachable!("VMMapArea should not be forked")
    }

    fn page_count(&self) -> usize {
        self.frames.len()
    }

    fn ubase(&self) -> usize {
        self.gbase
    }

    fn set_ubase(&mut self, _ubase: usize) {
        unreachable!("VMMapArea ubase should not be changed")
    }

    fn split(self: Box<Self>, _uaddr: usize) -> (Box<dyn Area>, Box<dyn Area>) {
        unreachable!("VMMapArea should not be split")
    }
}
