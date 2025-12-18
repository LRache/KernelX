use alloc::sync::Arc;
use alloc::vec::Vec;
use alloc::boxed::Box;
use spin::RwLock;

use crate::kernel::mm::AddrSpace;
use crate::kernel::mm::maparea::area::Area;
use crate::kernel::mm::{MapPerm, MemAccessType};
use crate::arch::{PageTable, PageTableTrait};
use crate::arch;

use super::nofilemap::FrameState;

#[cfg(feature = "swap-memory")]
use super::nofilemap::SwappableNoFileFrame;

pub struct AnonymousArea {
    ubase: usize,
    perm: MapPerm,
    frames: Vec<FrameState>,
    shared: bool
}

impl AnonymousArea {
    pub fn new(ubase: usize, perm: MapPerm, page_count: usize, shared: bool) -> Self {
        // Anonymous areas should be page-aligned
        debug_assert!(ubase % arch::PGSIZE == 0, "ubase should be page-aligned");
        
        let frames = Vec::from_iter((0..page_count).map(|_| FrameState::Unallocated));
        Self {
            ubase,
            perm,
            frames,
            shared
        }
    }

    fn allocate_page(&mut self, page_index: usize, addrspace: &AddrSpace) -> usize {
        debug_assert!(page_index < self.frames.len());
        debug_assert!(self.frames[page_index].is_unallocated());

        // Create a new zeroed page for anonymous memory
        let uaddr = self.ubase + page_index * arch::PGSIZE;
        let (allocated, kpage) = FrameState::allocate(uaddr, addrspace);

        addrspace.pagetable().write().mmap(uaddr, kpage, self.perm);
        self.frames[page_index] = allocated;

        kpage
    }

    fn copy_on_write_page(&mut self, page_index: usize, addrspace: &AddrSpace) -> usize {
        debug_assert!(page_index < self.frames.len());
        debug_assert!(self.frames[page_index].is_cow());

        let kpage = self.frames[page_index].cow_to_allocated(addrspace);

        addrspace.pagetable().write().mmap_replace(self.ubase + page_index * arch::PGSIZE, kpage, self.perm);

        kpage
    }

    #[cfg(feature = "swap-memory")]
    fn handle_memory_fault_on_swapped_allocated(&self, frame: &SwappableNoFileFrame, addrspace: &AddrSpace) {
        let page = frame.get_page_swap_in();
        // FIXME: if the page is swapped out again before we mmap, 
        // there could be issues
        addrspace.pagetable().write().mmap(frame.uaddr(), page, self.perm);
    }

    #[cfg(feature = "swap-memory")]
    fn handle_cow_read_swapped_out(&self, frame: &SwappableNoFileFrame, addrspace: &AddrSpace) {
        debug_assert!(frame.is_swapped_out(), "Frame is not swapped out");
        let kpage = frame.get_page_swap_in();
        addrspace.pagetable().write().mmap(
            frame.uaddr(), 
            kpage, 
            self.perm - MapPerm::W
        );
    }
}

impl Area for AnonymousArea {
    fn translate_read(&mut self, uaddr: usize, addrspace: &AddrSpace) -> Option<usize> {
        debug_assert!(uaddr >= self.ubase);

        let page_index = (uaddr - self.ubase) / arch::PGSIZE;
        let page_offset = (uaddr - self.ubase) % arch::PGSIZE;
        
        if let Some(page_frame) = self.frames.get(page_index) {
            let page = match page_frame {
                FrameState::Unallocated => {
                    self.allocate_page(page_index, addrspace)
                }
                FrameState::Allocated(frame) | FrameState::Cow(frame) => {
                    frame.get_page_swap_in()
                }
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
                FrameState::Unallocated => {
                    self.allocate_page(page_index, addrspace)
                }
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

    fn fork(&mut self, self_pagetable: &RwLock<PageTable>, new_pagetable: &RwLock<PageTable>) -> Box<dyn Area> {
        let perm = self.perm - MapPerm::W;
        let mut new_pagetable = new_pagetable.write();
        let frames = self.frames.iter().enumerate().map(|(page_index, frame)| {
            match frame {
                FrameState::Unallocated => FrameState::Unallocated,
                FrameState::Allocated(frame) => {
                    if self.shared {
                        if let Some(kpage) = frame.get_page() {
                            new_pagetable.mmap(
                                self.ubase + page_index * arch::PGSIZE,
                                kpage,
                                self.perm
                            );
                        }
                        FrameState::Allocated(frame.clone())
                    } else {
                        if let Some(kpage) = frame.get_page() {
                            new_pagetable.mmap(
                                self.ubase + page_index * arch::PGSIZE,
                                kpage,
                                perm
                            );
                        }
                        FrameState::Cow(frame.clone())
                    }
                },
                FrameState::Cow(frame) => {
                    debug_assert!(!self.shared, "Shared frames should not be CoW");
                    if let Some(kpage) = frame.get_page() {
                        new_pagetable.mmap(
                            self.ubase + page_index * arch::PGSIZE,
                            kpage,
                            perm
                        );
                    }
                    FrameState::Cow(frame.clone())
                }
            }
        }).collect();

        if !self.shared {
            let mut self_pagetable = self_pagetable.write();
            self.frames.iter_mut().enumerate().for_each(|(page_index, frame)| {
                match frame {
                    FrameState::Allocated(allocated) => {
                        if let Some(_) = allocated.get_page() {
                            if self.perm.contains(MapPerm::W) {
                                self_pagetable.mmap_replace_perm(
                                    self.ubase + page_index * arch::PGSIZE,
                                    perm
                                );
                            }
                        }
                        *frame = FrameState::Cow(allocated.clone());
                    },
                    _ => {}
                }
            });
        }

        let new_area = AnonymousArea {
            ubase: self.ubase,
            perm: self.perm,
            frames,
            shared: self.shared
        };

        Box::new(new_area)
    }

    #[allow(unused_variables)]
    fn try_to_fix_memory_fault(&mut self, uaddr: usize, access_type: MemAccessType, addrspace: &Arc<AddrSpace>) -> bool {
        debug_assert!(uaddr >= self.ubase);

        let page_index = (uaddr - self.ubase) / arch::PGSIZE;
        if page_index < self.frames.len() {
            match &self.frames[page_index] {
                FrameState::Unallocated => {
                    self.allocate_page(page_index, addrspace);
                }
                FrameState::Allocated(frame) => {
                    #[cfg(feature = "swap-memory")]
                    self.handle_memory_fault_on_swapped_allocated(frame, addrspace);
                    
                    #[cfg(not(feature = "swap-memory"))]
                    // Page is already allocated, this shouldn't happen
                    panic!("Memory fault on already allocated page at address: {:#x}, access_type: {:?}, perm: {:?}", uaddr, access_type, self.perm);
                }
                FrameState::Cow(frame) => {
                    if access_type != MemAccessType::Write {
                        #[cfg(feature = "swap-memory")]
                        self.handle_cow_read_swapped_out(frame, addrspace);
                        #[cfg(not(feature = "swap-memory"))]
                        panic!("Memory fault on CoW page without write access at address: {:#x}, access_type: {:?}, perm: {:?}", uaddr, access_type, self.perm);
                    } else {
                        self.copy_on_write_page(page_index, addrspace);
                    }
                }
            }
            
            true
        } else {
            false
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
        debug_assert!(uaddr >= self.ubase && uaddr < self.ubase + self.size(), "uaddr out of range for split, urange: [{:#x}, {:#x}), uaddr: {:#x}", self.ubase, self.ubase + self.size(), uaddr);

        let split_index = (uaddr - self.ubase) / arch::PGSIZE;
        let new_ubase = self.ubase + split_index * arch::PGSIZE;
        
        let new_frames = self.frames.split_off(split_index);
        let new_area = AnonymousArea {
            ubase: new_ubase,
            perm: self.perm,
            frames: new_frames,
            shared: self.shared
        };

        (
            self, 
            Box::new(new_area)
        )
    }

    fn ubase(&self) -> usize {
        self.ubase
    }

    fn set_perm(&mut self, perm: MapPerm, pagetable: &RwLock<PageTable>) {
        if perm == self.perm {
            return;
        }
        
        self.perm = perm;

        let mut pagetable = pagetable.write();
        for frame in self.frames.iter() {
            if let FrameState::Allocated(frame) | FrameState::Cow(frame) = frame {
                if !frame.is_swapped_out() {
                    pagetable.mmap_replace_perm(frame.uaddr(), perm);
                }
            }
            // Note: We don't update COW pages here as they should maintain
            // their current permission state until the next write access
        }
    }

    fn unmap(&mut self, pagetable: &RwLock<PageTable>) {
        let mut pagetable = pagetable.write();
        for frame in self.frames.iter_mut() {
            #[cfg(feature = "swap-memory")]
            if let FrameState::Allocated(frame) | FrameState::Cow(frame) = frame {
                if !frame.is_swapped_out() {
                    pagetable.munmap(frame.uaddr());
                }
            }
            #[cfg(not(feature = "swap-memory"))]
            if let FrameState::Allocated(frame) | FrameState::Cow(frame) = frame {
                pagetable.munmap(frame.uaddr());
            }
            *frame = FrameState::Unallocated;
        }
    }

    fn type_name(&self) -> &'static str {
        "anonymous"
    }
}
