use alloc::sync::Arc;
use alloc::vec::Vec;
use alloc::boxed::Box;
use spin::RwLock;

use crate::arch::{PageTable, PageTableTrait};
use crate::arch;
use crate::kernel::mm::PhysPageFrame;
use crate::kernel::mm::maparea::area::Area;
use crate::kernel::mm::maparea::nofilemap::{FrameState, SwappableNoFileFrame};
use crate::kernel::mm::{AddrSpace, MapPerm, MemAccessType};
use crate::fs::file::File;

pub struct PrivateFileMapArea {
    ubase: usize,
    perm: MapPerm,
    
    file: Arc<File>,
    file_offset: usize,
    file_length: usize,
    
    frames: Vec<FrameState>,
}

impl PrivateFileMapArea {
    pub fn new(
        ubase: usize,
        perm: MapPerm,
        file: Arc<File>, 
        file_offset: usize,
        file_length: usize
    ) -> Self {
        // File mapping areas should be page-aligned
        debug_assert!(ubase % arch::PGSIZE == 0, "ubase should be page-aligned");
        debug_assert!(file_offset % arch::PGSIZE == 0, "file_offset should be page-aligned");

        let page_count = (file_length + arch::PGSIZE - 1) / arch::PGSIZE;
        let frames = Vec::from_iter((0..page_count).map(|_| FrameState::Unallocated));
        Self {
            ubase,
            perm,
            file,
            file_offset,
            file_length,
            frames,
        }
    }

    fn load_page(&mut self, page_index: usize, addrspace: &AddrSpace) -> usize {
        debug_assert!(page_index < self.frames.len());
        debug_assert!(self.frames[page_index].is_unallocated());

        let area_offset = page_index * arch::PGSIZE;
        let file_offset = self.file_offset + area_offset;

        let uaddr = self.ubase + area_offset;
        
        let frame = PhysPageFrame::alloc_zeroed();
        
        // Try to read from file, but only within the specified file_length
        if area_offset < self.file_length {
            let mut buffer = [0u8; arch::PGSIZE];
            // Calculate how much data we can read from this page:
            // - Don't read beyond the file_length boundary
            // - Don't read beyond one page
            let length = core::cmp::min(self.file_length - area_offset, arch::PGSIZE);
            
            match self.file.read_at(&mut buffer[..length], file_offset) {
                Ok(_) => {
                    frame.copy_from_slice(0, &buffer[..length]);
                }
                Err(_) => {
                    // Keep the page zeroed if file read fails
                }
            }
        }

        let kpage = frame.get_page();

        addrspace.pagetable().write().mmap(self.ubase + area_offset, kpage, self.perm);
        self.frames[page_index] = FrameState::Allocated(Arc::new(SwappableNoFileFrame::allocated(uaddr, frame, addrspace)));

        kpage
    }

    fn copy_on_write_page(&mut self, page_index: usize, addrspace: &AddrSpace) -> usize {
        debug_assert!(page_index < self.frames.len());
        
        debug_assert!(self.perm.contains(MapPerm::W), "Original mapping must have write permission for copy-on-write");

        let area_offset = page_index * arch::PGSIZE;
        let (frame, kpage) = match &self.frames[page_index] {
            FrameState::Cow(frame) => frame.copy(addrspace),
            _ => panic!("Invalid type for copy-on-write"),
        };

        addrspace.pagetable().write().mmap_replace_kaddr(self.ubase + area_offset, kpage);
        self.frames[page_index] = FrameState::Allocated(Arc::new(frame));

        kpage
    }

    #[cfg(feature = "swap-memory")]
    fn handle_memory_fault_on_swapped_allocated(&self, frame: &SwappableNoFileFrame, addrspace: &AddrSpace) {
        debug_assert!(frame.is_swapped_out(), "FrameState is not swapped out");
        let kpage = frame.get_page_swap_in();
        addrspace.pagetable().write().mmap(
            frame.uaddr(),
            kpage,
            self.perm,
        );
    }
}

impl Area for PrivateFileMapArea {
    fn translate_read(&mut self, uaddr: usize, addrspace: &Arc<AddrSpace>) -> Option<usize> {
        assert!(uaddr >= self.ubase);

        let page_index = (uaddr - self.ubase) / arch::PGSIZE;
        let page_offset = (uaddr - self.ubase) % arch::PGSIZE;
        
        if let Some(page_frame) = self.frames.get(page_index) {
            let page = match page_frame {
                FrameState::Unallocated => {
                    // Lazy loading: load page from file on first access
                    self.load_page(page_index, addrspace)
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

    fn translate_write(&mut self, uaddr: usize, addrspace: &Arc<AddrSpace>) -> Option<usize> {
        debug_assert!(uaddr >= self.ubase);

        if !self.perm.contains(MapPerm::W) {
            return None;
        }

        let page_index = (uaddr - self.ubase) / arch::PGSIZE;
        let page_offset = (uaddr - self.ubase) % arch::PGSIZE;
        
        if let Some(page_frame) = self.frames.get_mut(page_index) {
            let page = match page_frame {
                FrameState::Unallocated => {
                    // Lazy loading: load page from file on first write
                    self.load_page(page_index, addrspace)
                }
                FrameState::Allocated(frame) => {
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
        let cow_perm = self.perm - MapPerm::W;
        
        let mut pagetable = new_pagetable.write();
        let frames = self.frames.iter().enumerate().map(|(page_index, frame)| {
            match frame {
                FrameState::Unallocated => FrameState::Unallocated,
                FrameState::Allocated(frame) | FrameState::Cow(frame) => {
                    if let Some(kpage) = frame.get_page() {
                        let uaddr = self.ubase + page_index * arch::PGSIZE;
                        pagetable.mmap(uaddr, kpage, cow_perm);
                    }
                    FrameState::Cow(frame.clone())
                }
            }
        }).collect();

        // Update original mapping to be COW
        let mut self_pagetable = self_pagetable.write();
        self.frames.iter_mut().enumerate().for_each(|(page_index, frame)| {
            if matches!(frame, FrameState::Allocated(_)) {
                if let FrameState::Allocated(f) = core::mem::replace(frame, FrameState::Unallocated) {
                    if let Some(_) = f.get_page() {
                        let uaddr = self.ubase + page_index * arch::PGSIZE;
                        self_pagetable.mmap_replace_perm(uaddr, cow_perm);
                    }
                    *frame = FrameState::Cow(f);
                }
            }
        });

        let new_area = PrivateFileMapArea {
            ubase: self.ubase,
            perm: self.perm,
            file: self.file.clone(),
            file_offset: self.file_offset,
            file_length: self.file_length,
            frames,
        };

        Box::new(new_area)
    }

    fn try_to_fix_memory_fault(&mut self, uaddr: usize, access_type: MemAccessType, addrspace: &Arc<AddrSpace>) -> bool {
        debug_assert!(uaddr >= self.ubase);

        if !self.perm.contains(MapPerm::W) && access_type == MemAccessType::Write {
            return false;
        }
        
        let page_index = (uaddr - self.ubase) / arch::PGSIZE;
        if page_index < self.frames.len() {
            match &self.frames[page_index] {
                FrameState::Unallocated => {
                    self.load_page(page_index, addrspace);
                }
                FrameState::Allocated(allocated) => {
                    // Page is already allocated, this shouldn't happen
                    #[cfg(feature = "swap-memory")]
                    self.handle_memory_fault_on_swapped_allocated(&allocated, addrspace);
                    #[cfg(not(feature = "swap-memory"))]
                    panic!("Memory fault on already allocated file page at address: {:#x}", uaddr);
                }
                FrameState::Cow(_) => {
                    assert!(access_type == MemAccessType::Write, "Memory fault on CoW file page for read access at address: {:#x}", uaddr);
                    self.copy_on_write_page(page_index, addrspace);
                }
            }
            
            true
        } else {
            false
        }
    }

    fn ubase(&self) -> usize {
        self.ubase
    }

    fn set_ubase(&mut self, ubase: usize) {
        self.ubase = ubase;
    }

    fn page_count(&self) -> usize {
        self.frames.len()
    }

    fn split(mut self: Box<Self>, uaddr: usize) -> (Box<dyn Area>, Box<dyn Area>) {
        debug_assert!(uaddr % arch::PGSIZE == 0, "Split address must be page-aligned");
        debug_assert!(uaddr > self.ubase, "Split address must be greater than ubase");
        debug_assert!(uaddr < self.ubase + self.size(), "Split address out of bounds");

        let split_index = (uaddr - self.ubase) / arch::PGSIZE;
        let split_offset = split_index * arch::PGSIZE;
        let remaining_frames = self.frames.split_off(split_index);

        // Calculate the new file_length for the second area
        let new_file_length = if self.file_length > split_offset {
            self.file_length - split_offset
        } else {
            0
        };

        // Update the file_length for the first area (self)
        self.file_length = core::cmp::min(self.file_length, split_offset);

        let new_area = PrivateFileMapArea {
            ubase: uaddr,
            perm: self.perm,
            file: self.file.clone(),
            file_offset: self.file_offset + split_offset,
            file_length: new_file_length,
            frames: remaining_frames,
        };

        (self, Box::new(new_area))
    }

    fn set_perm(&mut self, perm: MapPerm, pagetable: &RwLock<PageTable>) {
        self.perm = perm;
        
        // Update page table permissions for all allocated pages
        let mut pagetable = pagetable.write();
        for (page_index, frame) in self.frames.iter().enumerate() {
            if let FrameState::Allocated(frame) = frame {
                if let Some(_) = frame.get_page() {
                    let uaddr = self.ubase + page_index * arch::PGSIZE;
                    pagetable.mmap_replace_perm(uaddr, perm);
                }
            }
        }
    }

    fn unmap(&mut self, pagetable: &RwLock<PageTable>) {
        let mut pagetable = pagetable.write();
        for (page_index, frame) in self.frames.iter_mut().enumerate() {
            if let FrameState::Allocated(_) | FrameState::Cow(_) = frame {
                let uaddr = self.ubase + page_index * arch::PGSIZE;
                pagetable.munmap(uaddr);
            }
            *frame = FrameState::Unallocated;
        }
    }

    fn type_name(&self) -> &'static str {
        "PrivateFileMapArea"
    }
}
