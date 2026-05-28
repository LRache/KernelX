use alloc::boxed::Box;
use alloc::sync::Arc;
use alloc::vec::Vec;

use crate::arch;
use crate::arch::{PageTable, PageTableTrait};
use crate::kernel::mm::maparea::area::{Area, Frame, MapAreaInfo, MemoryFaultSignal};
use crate::kernel::mm::{AddrSpace, MapPerm, MemAccessType, PhysPageFrame};
use crate::klib::SpinLock;

type SharedFrames = Arc<SpinLock<Vec<SpinLock<Frame>>>>;

pub struct SharedAnonymousArea {
    ubase: usize,
    perm: MapPerm,
    frames: SharedFrames,
}

impl SharedAnonymousArea {
    pub fn new(ubase: usize, perm: MapPerm, page_count: usize) -> Self {
        debug_assert!(ubase % arch::PGSIZE == 0, "ubase should be page-aligned");

        let frames =
            Vec::from_iter((0..page_count).map(|_| SpinLock::new(Frame::Unallocated, "SharedAnonymous::frames")));
        Self {
            ubase,
            perm,
            frames: Arc::new(SpinLock::new(frames, "SharedAnonymous::frames")),
        }
    }

    fn allocate_page(uaddr: usize, perm: MapPerm, frame: &SpinLock<Frame>, addrspace: &AddrSpace) -> usize {
        let page_frame = PhysPageFrame::alloc_zeroed();
        let kpage = page_frame.get_page();
        addrspace.pagetable().lock().mmap(uaddr, kpage, perm);
        *frame.lock() = Frame::Allocated(Arc::new(page_frame));
        kpage
    }
}

impl Area for SharedAnonymousArea {
    fn translate_read(&mut self, uaddr: usize, addrspace: &AddrSpace) -> Option<usize> {
        debug_assert!(uaddr >= self.ubase);

        let page_index = (uaddr - self.ubase) / arch::PGSIZE;
        let page_offset = (uaddr - self.ubase) % arch::PGSIZE;

        let frames = self.frames.lock();
        let frame_lock = frames.get(page_index)?;

        let kpage = {
            let frame = frame_lock.lock();
            match &*frame {
                Frame::Allocated(f) => f.get_page(),
                Frame::Unallocated => {
                    drop(frame);
                    Self::allocate_page(self.ubase + page_index * arch::PGSIZE, self.perm, frame_lock, addrspace)
                }
                Frame::Cow(_) => unreachable!("SharedAnonymousArea should not have CoW frames"),
            }
        };

        Some(kpage + page_offset)
    }

    fn translate_write(&mut self, uaddr: usize, addrspace: &AddrSpace) -> Option<usize> {
        debug_assert!(uaddr >= self.ubase);

        if !self.perm.contains(MapPerm::W) {
            return None;
        }

        let page_index = (uaddr - self.ubase) / arch::PGSIZE;
        let page_offset = (uaddr - self.ubase) % arch::PGSIZE;

        let frames = self.frames.lock();
        let frame_lock = frames.get(page_index)?;

        let kpage = {
            let frame = frame_lock.lock();
            match &*frame {
                Frame::Allocated(f) => f.get_page(),
                Frame::Unallocated => {
                    drop(frame);
                    Self::allocate_page(self.ubase + page_index * arch::PGSIZE, self.perm, frame_lock, addrspace)
                }
                Frame::Cow(_) => unreachable!("SharedAnonymousArea should not have CoW frames"),
            }
        };

        Some(kpage + page_offset)
    }

    fn perm(&self) -> MapPerm {
        self.perm
    }

    fn ubase(&self) -> usize {
        self.ubase
    }

    fn set_ubase(&mut self, ubase: usize) {
        debug_assert!(ubase % arch::PGSIZE == 0, "ubase should be page-aligned");
        self.ubase = ubase;
    }

    fn page_count(&self) -> usize {
        self.frames.lock().len()
    }

    fn fork(&mut self, _self_pagetable: &SpinLock<PageTable>, new_pagetable: &mut PageTable) -> Box<dyn Area> {
        // Shared: fork just clones the Arc, both processes see the same frames
        let frames = self.frames.lock();
        for (page_index, frame_lock) in frames.iter().enumerate() {
            let frame = frame_lock.lock();
            if let Frame::Allocated(f) = &*frame {
                new_pagetable.mmap(self.ubase + page_index * arch::PGSIZE, f.get_page(), self.perm);
            }
        }
        drop(frames);

        Box::new(SharedAnonymousArea {
            ubase: self.ubase,
            perm: self.perm,
            frames: self.frames.clone(),
        })
    }

    fn try_to_fix_memory_fault(
        &mut self,
        uaddr: usize,
        _access_type: MemAccessType,
        addrspace: &AddrSpace,
    ) -> Result<usize, MemoryFaultSignal> {
        debug_assert!(uaddr >= self.ubase);

        let page_index = (uaddr - self.ubase) / arch::PGSIZE;
        let page_offset = (uaddr - self.ubase) % arch::PGSIZE;
        let frames = self.frames.lock();

        if let Some(frame_lock) = frames.get(page_index) {
            let kpage = {
                let frame = frame_lock.lock();
                match &*frame {
                    Frame::Unallocated => {
                        drop(frame);
                        Self::allocate_page(self.ubase + page_index * arch::PGSIZE, self.perm, frame_lock, addrspace)
                    }
                    Frame::Allocated(f) => {
                        let kpage = f.get_page();
                        addrspace
                            .pagetable()
                            .lock()
                            .mmap(uaddr & !(arch::PGSIZE - 1), kpage, self.perm);
                        kpage
                    }
                    Frame::Cow(_) => unreachable!("SharedAnonymousArea should not have CoW frames"),
                }
            };
            Ok(kpage + page_offset)
        } else {
            Err(MemoryFaultSignal::Segv)
        }
    }

    fn split(self: Box<Self>, uaddr: usize) -> (Box<dyn Area>, Box<dyn Area>) {
        debug_assert!(uaddr % arch::PGSIZE == 0, "uaddr should be page-aligned");
        debug_assert!(
            uaddr >= self.ubase && uaddr < self.ubase + self.size(),
            "uaddr out of range for split"
        );

        let split_index = (uaddr - self.ubase) / arch::PGSIZE;
        let new_ubase = self.ubase + split_index * arch::PGSIZE;

        let mut all_frames = self.frames.lock();
        let new_frames_vec = all_frames.split_off(split_index);
        let left_frames_vec = core::mem::take(&mut *all_frames);
        drop(all_frames);

        let left = SharedAnonymousArea {
            ubase: self.ubase,
            perm: self.perm,
            frames: Arc::new(SpinLock::new(left_frames_vec, "shared_anon_frames")),
        };

        let right = SharedAnonymousArea {
            ubase: new_ubase,
            perm: self.perm,
            frames: Arc::new(SpinLock::new(new_frames_vec, "shared_anon_frames")),
        };

        (Box::new(left), Box::new(right))
    }

    fn set_perm(&mut self, perm: MapPerm, pagetable: &SpinLock<PageTable>) {
        if perm == self.perm {
            return;
        }

        self.perm = perm;

        let mut pagetable = pagetable.lock();
        let frames = self.frames.lock();
        for (i, frame_lock) in frames.iter().enumerate() {
            let frame = frame_lock.lock();
            if let Frame::Allocated(_) = &*frame {
                pagetable.mmap_replace_perm(self.ubase + i * arch::PGSIZE, perm);
            }
        }
    }

    fn unmap(&mut self, pagetable: &SpinLock<PageTable>) {
        let mut pagetable = pagetable.lock();
        let frames = self.frames.lock();
        for (i, p) in frames.iter().enumerate() {
            // Only unmap from this process's page table; don't touch the shared frames
            let p = p.lock();
            if !p.is_unallocated() {
                // The page may not be mapped to the page table if it was loaded by 
                // `translate_read` or `translate_write` but never accessed afterwards.
                let _ = pagetable.munmap(self.ubase + i * arch::PGSIZE);
            }
        }
    }

    fn type_name(&self) -> &'static str {
        "shared-anonymous"
    }

    fn map_area_info(&self) -> MapAreaInfo {
        let mut info = MapAreaInfo::new(self.ubase(), self.ubase() + self.size(), self.perm);
        info.shared = true;
        info
    }
}
