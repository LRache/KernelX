use alloc::boxed::Box;
use alloc::vec::Vec;

use crate::arch;
use crate::arch::{PageTable, PageTableTrait};
use crate::kernel::mm::maparea::area::{Area, MemoryFaultSignal};
use crate::kernel::mm::maparea::nofilemap::{FrameState, SwappableNoFileFrame};
use crate::kernel::mm::{AddrSpace, MapPerm, MemAccessType};
use crate::klib::SpinLock;

pub struct PrivateAnonymousArea {
    ubase: usize,
    perm: MapPerm,
    frames: Vec<FrameState>,
}

impl PrivateAnonymousArea {
    pub fn new(ubase: usize, perm: MapPerm, page_count: usize) -> Self {
        // Anonymous areas should be page-aligned
        debug_assert!(ubase % arch::PGSIZE == 0, "ubase should be page-aligned");

        let frames = Vec::from_iter((0..page_count).map(|_| FrameState::Unallocated));
        Self { ubase, perm, frames }
    }

    fn allocate_page(&mut self, page_index: usize, addrspace: &AddrSpace) -> usize {
        debug_assert!(page_index < self.frames.len());
        debug_assert!(self.frames[page_index].is_unallocated());

        // Create a new zeroed page for anonymous memory
        let uaddr = self.ubase + page_index * arch::PGSIZE;
        let (allocated, kpage) = FrameState::allocate(uaddr, addrspace);

        addrspace.pagetable().lock().mmap(uaddr, kpage, self.perm);
        self.frames[page_index] = allocated;

        kpage
    }

    fn copy_on_write_page(&mut self, page_index: usize, addrspace: &AddrSpace) -> usize {
        debug_assert!(page_index < self.frames.len());
        debug_assert!(self.frames[page_index].is_cow());

        let kpage = self.frames[page_index].cow_to_allocated(addrspace);
        let uaddr = self.ubase + page_index * arch::PGSIZE;

        addrspace.pagetable().lock().mmap_replace(uaddr, kpage, self.perm);
        addrspace.notify_addrspace_remap(uaddr, 1);

        kpage
    }

    fn map_cow_page(&self, page_index: usize, frame: &SwappableNoFileFrame, addrspace: &AddrSpace) -> usize {
        let kpage = frame.get_page_swap_in();
        let uaddr = self.ubase + page_index * arch::PGSIZE;

        addrspace
            .pagetable()
            .lock()
            .mmap_replace(uaddr, kpage, self.perm - MapPerm::W);

        kpage
    }

    #[cfg(feature = "swap-memory")]
    fn handle_memory_fault_on_swapped_allocated(&self, frame: &SwappableNoFileFrame, addrspace: &AddrSpace) -> usize {
        let page = frame.get_page_swap_in();
        // FIXME: if the page is swapped out again before we mmap,
        // there could be issues
        addrspace.pagetable().write().mmap(frame.uaddr(), page, self.perm);
        page
    }
}

impl Area for PrivateAnonymousArea {
    fn translate_read(&mut self, uaddr: usize, addrspace: &AddrSpace) -> Option<usize> {
        debug_assert!(uaddr >= self.ubase);

        let page_index = (uaddr - self.ubase) / arch::PGSIZE;
        let page_offset = (uaddr - self.ubase) % arch::PGSIZE;

        if let Some(page_frame) = self.frames.get(page_index) {
            let page = match page_frame {
                FrameState::Unallocated => self.allocate_page(page_index, addrspace),
                FrameState::Allocated(frame) | FrameState::Cow(frame) => frame.get_page_swap_in(),
            };

            Some(page + page_offset)
        } else {
            None
        }
    }

    fn translate_write(&mut self, uaddr: usize, addrspace: &AddrSpace) -> Option<usize> {
        assert!(uaddr >= self.ubase);

        if !self.perm.contains(MapPerm::W) {
            return None;
        }

        let page_index = (uaddr - self.ubase) / arch::PGSIZE;
        let page_offset = (uaddr - self.ubase) % arch::PGSIZE;

        if let Some(page_frame) = self.frames.get_mut(page_index) {
            let page = match page_frame {
                FrameState::Unallocated => self.allocate_page(page_index, addrspace),
                FrameState::Allocated(frame) => {
                    // frame_get_page_swapped(frame)
                    frame.get_page_swap_in()
                }
                FrameState::Cow(_) => {
                    // Copy-on-write: create a new copy for this process
                    self.copy_on_write_page(page_index, addrspace)
                }
            };

            Some(page + page_offset)
        } else {
            None
        }
    }

    fn perm(&self) -> MapPerm {
        self.perm
    }

    fn fork(&mut self, self_pagetable: &SpinLock<PageTable>) -> Box<dyn Area> {
        let perm = self.perm - MapPerm::W;

        let frames = self
            .frames
            .iter()
            .map(|frame| match frame {
                FrameState::Unallocated => FrameState::Unallocated,
                FrameState::Allocated(frame) | FrameState::Cow(frame) => FrameState::Cow(frame.clone()),
            })
            .collect();

        let mut self_pagetable = self_pagetable.lock();
        self.frames
            .iter_mut()
            .enumerate()
            .for_each(|(page_index, frame)| match frame {
                FrameState::Allocated(allocated) => {
                    if let Some(_) = allocated.get_page() {
                        let uaddr = self.ubase + page_index * arch::PGSIZE;
                        if self.perm.contains(MapPerm::W) && self_pagetable.mapped_flag(uaddr).is_some() {
                            self_pagetable.mmap_replace_perm(uaddr, perm);
                        }
                    }
                    *frame = FrameState::Cow(allocated.clone());
                }
                _ => {}
            });

        let new_area = Self {
            ubase: self.ubase,
            perm: self.perm,
            frames,
        };

        Box::new(new_area)
    }

    #[allow(unused_variables)]
    fn try_to_fix_memory_fault(
        &mut self,
        uaddr: usize,
        access_type: MemAccessType,
        addrspace: &AddrSpace,
    ) -> Result<usize, MemoryFaultSignal> {
        debug_assert!(uaddr >= self.ubase);

        let page_index = (uaddr - self.ubase) / arch::PGSIZE;
        if page_index < self.frames.len() {
            let page_offset = (uaddr - self.ubase) % arch::PGSIZE;
            let page = match &self.frames[page_index] {
                FrameState::Unallocated => self.allocate_page(page_index, addrspace),
                FrameState::Allocated(frame) => {
                    #[cfg(feature = "swap-memory")]
                    {
                        self.handle_memory_fault_on_swapped_allocated(frame, addrspace)
                    }
                    #[cfg(not(feature = "swap-memory"))]
                    {
                        frame.get_page_swap_in()
                    }
                }
                FrameState::Cow(frame) => {
                    if access_type != MemAccessType::Write {
                        self.map_cow_page(page_index, frame, addrspace)
                    } else {
                        self.copy_on_write_page(page_index, addrspace)
                    }
                }
            };
            Ok(page + page_offset)
        } else {
            Err(MemoryFaultSignal::Segv)
        }
    }

    fn set_ubase(&mut self, ubase: usize) {
        debug_assert!(ubase % arch::PGSIZE == 0, "ubase should be page-aligned");
        self.ubase = ubase;
    }

    fn page_count(&self) -> usize {
        self.frames.len()
    }

    fn split(mut self: Box<Self>, uaddr: usize) -> (Box<dyn Area>, Box<dyn Area>) {
        debug_assert!(uaddr % arch::PGSIZE == 0, "uaddr should be page-aligned");
        debug_assert!(
            uaddr >= self.ubase && uaddr < self.ubase + self.size(),
            "uaddr out of range for split, urange: [{:#x}, {:#x}), uaddr: {:#x}",
            self.ubase,
            self.ubase + self.size(),
            uaddr
        );

        let split_index = (uaddr - self.ubase) / arch::PGSIZE;
        let new_ubase = self.ubase + split_index * arch::PGSIZE;

        let new_frames = self.frames.split_off(split_index);
        let new_area = Self {
            ubase: new_ubase,
            perm: self.perm,
            frames: new_frames,
        };

        (self, Box::new(new_area))
    }

    fn ubase(&self) -> usize {
        self.ubase
    }

    fn set_perm(&mut self, perm: MapPerm, pagetable: &SpinLock<PageTable>) {
        if perm == self.perm {
            return;
        }

        self.perm = perm;

        let mut pagetable = pagetable.lock();
        for frame in self.frames.iter() {
            match frame {
                FrameState::Allocated(frame) => {
                    if !frame.is_swapped_out() && pagetable.mapped_flag(frame.uaddr()).is_some() {
                        pagetable.mmap_replace_perm(frame.uaddr(), perm);
                    }
                }
                FrameState::Cow(frame) => {
                    if !frame.is_swapped_out() && pagetable.mapped_flag(frame.uaddr()).is_some() {
                        pagetable.mmap_replace_perm(frame.uaddr(), perm - MapPerm::W);
                    }
                }
                FrameState::Unallocated => {}
            }
        }
    }

    fn unmap(&mut self, pagetable: &SpinLock<PageTable>) {
        let mut pagetable = pagetable.lock();
        for frame in self.frames.iter_mut() {
            #[cfg(feature = "swap-memory")]
            if let FrameState::Allocated(frame) | FrameState::Cow(frame) = frame {
                if !frame.is_swapped_out() {
                    pagetable.munmap(frame.uaddr());
                }
            }
            #[cfg(not(feature = "swap-memory"))]
            if let FrameState::Allocated(frame) | FrameState::Cow(frame) = frame {
                // The page may not be mapped to the page table if it was loaded by `translate_read` or `translate_write` but never accessed afterwards.
                let _ = pagetable.munmap(frame.uaddr());
            }
            *frame = FrameState::Unallocated;
        }
    }

    fn type_name(&self) -> &'static str {
        "private-anonymous"
    }
}
