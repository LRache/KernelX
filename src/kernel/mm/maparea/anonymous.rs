use alloc::sync::Arc;
use alloc::vec::Vec;
use alloc::boxed::Box;
use spin::RwLock;

use crate::kernel::mm::{AddrSpace, PhysPageFrame};
use crate::kernel::mm::maparea::area::Area;
use crate::kernel::mm::{MapPerm, MemAccessType};
use crate::arch::{PageTable, PageTableTrait};
use crate::arch;

cfg_if::cfg_if! {
    if #[cfg(feature="swap-memory")] {
        use crate::kernel::mm::swappable;
        type PageFrame = swappable::AnonymousFrame;
    } else {
        struct PageFrame {
            frame: PhysPageFrame,
            uaddr: usize,
        }
    }
}

fn frame_get_page(frame: &PageFrame) -> usize {
    #[cfg(feature = "swap-memory")]
    {
        if let Some(page) = frame.page() {
            page
        } else {
            frame.swap_in()
            // panic!("Trying to get page of a swapped-out anonymous frame");
        }
    }
    #[cfg(not(feature = "swap-memory"))]
    {
        frame.frame.get_page()
    }
}

fn alloc_frame(_addrspace: &AddrSpace, uaddr: usize) -> PageFrame {
    #[cfg(feature = "swap-memory")]
    {
        use crate::kernel::mm::swappable::AnonymousFrame;

        AnonymousFrame::alloc(_uaddr, _addrspace.family_chain().clone())
    }
    #[cfg(not(feature = "swap-memory"))]
    {
        PageFrame {
            frame: PhysPageFrame::alloc_zeroed(),
            uaddr,
        }
    }
}

fn copy_frame(frame: &PageFrame, _addrspace: &AddrSpace) -> PageFrame {
    #[cfg(feature = "swap-memory")]
    {
        frame.copy(_addrspace.family_chain().clone())
    }
    #[cfg(not(feature = "swap-memory"))]
    {
        PageFrame { frame: frame.frame.copy(), uaddr: frame.uaddr }
    }
}

enum Frame {
    Unallocated,
    Allocated(Arc<PageFrame>),
    Cow(Arc<PageFrame>),
}

impl Frame {
    fn is_unallocated(&self) -> bool {
        matches!(self, Frame::Unallocated)
    }

    fn is_cow(&self) -> bool {
        matches!(self, Frame::Cow(_))
    }
}

pub struct AnonymousArea {
    ubase: usize,
    perm: MapPerm,
    frames: Vec<Frame>,
}

impl AnonymousArea {
    pub fn new(ubase: usize, perm: MapPerm, page_count: usize) -> Self {
        // Anonymous areas should be page-aligned
        assert!(ubase % arch::PGSIZE == 0, "ubase should be page-aligned");
        
        let frames = Vec::from_iter((0..page_count).map(|_| Frame::Unallocated));
        Self {
            ubase,
            perm,
            frames
        }
    }

    fn allocate_page(&mut self, page_index: usize, addrspace: &Arc<AddrSpace>) -> usize {
        assert!(page_index < self.frames.len());
        assert!(self.frames[page_index].is_unallocated());

        // Create a new zeroed page for anonymous memory
        let uaddr = self.ubase + page_index * arch::PGSIZE;
        let frame = alloc_frame(addrspace, uaddr);
        let page = frame_get_page(&frame);

        addrspace.pagetable().write().mmap(uaddr, page, self.perm);
        self.frames[page_index] = Frame::Allocated(Arc::new(frame));

        page
    }

    fn copy_on_write_page(&mut self, page_index: usize, addrspace: &AddrSpace) -> usize {
        assert!(page_index < self.frames.len());
        assert!(self.frames[page_index].is_cow());

        let cow_frame = core::mem::replace(&mut self.frames[page_index], Frame::Unallocated);
        let new_frame = if let Frame::Cow(frame) = cow_frame {
            match Arc::try_unwrap(frame) {
                Ok(only) => only,
                Err(cow) => copy_frame(&cow, addrspace),
            }
        } else {
            unreachable!();
        };

        let page = frame_get_page(&new_frame);

        addrspace.pagetable().write().mmap_replace(self.ubase + page_index * arch::PGSIZE, page, self.perm);
        self.frames[page_index] = Frame::Allocated(Arc::new(new_frame));

        page
    }

    #[cfg(feature = "swap-memory")]
    fn handle_memory_fault_on_swap(&self, frame: &PageFrame, addrspace: &AddrSpace) {
        // TODO: Now we can't judge whether there
        // is a bug of mapping or a swapped-out page.
        // So we just swap in the page unconditionally.
        let page = frame.swap_in();
        addrspace.pagetable().write().mmap(frame.uaddr(), page, self.perm);
    }
}

impl Area for AnonymousArea {
    fn translate_read(&mut self, uaddr: usize, addrspace: &Arc<AddrSpace>) -> Option<usize> {
        debug_assert!(uaddr >= self.ubase);

        let page_index = (uaddr - self.ubase) / arch::PGSIZE;
        let page_offset = (uaddr - self.ubase) % arch::PGSIZE;
        
        if let Some(page_frame) = self.frames.get(page_index) {
            let page = match page_frame {
                Frame::Unallocated => {
                    // Lazy allocation: allocate page on first access
                    self.allocate_page(page_index, addrspace)
                }
                Frame::Allocated(frame) => {
                    frame_get_page(frame)
                }
                Frame::Cow(frame) => {
                    frame_get_page(frame)
                }
            };

            Some(page + page_offset)
        } else {
            None
        }
    }

    fn translate_write(&mut self, uaddr: usize, addrspace: &Arc<AddrSpace>) -> Option<usize> {
        assert!(uaddr >= self.ubase);

        if !self.perm.contains(MapPerm::W) {
            return None;
        }

        let page_index = (uaddr - self.ubase) / arch::PGSIZE;
        let page_offset = (uaddr - self.ubase) % arch::PGSIZE;
        
        if let Some(page_frame) = self.frames.get_mut(page_index) {
            let page = match page_frame {
                Frame::Unallocated => {
                    self.allocate_page(page_index, addrspace)
                }
                Frame::Allocated(frame) => {
                    frame_get_page(frame)
                }
                Frame::Cow(_) => {
                    // Copy-on-write: create a new copy for this process
                    self.copy_on_write_page(page_index, addrspace)
                }
            };

            Some(page + page_offset)
        } else {
            None
        }
    }

    fn fork(&mut self, self_pagetable: &RwLock<PageTable>, new_pagetable: &RwLock<PageTable>) -> Box<dyn Area> {
        let perm = self.perm - MapPerm::W;
        let mut new_pagetable = new_pagetable.write();
        let frames = self.frames.iter().enumerate().map(|(page_index, frame)| {
            match frame {
                Frame::Unallocated => Frame::Unallocated,
                Frame::Allocated(frame) | Frame::Cow(frame) => {
                    new_pagetable.mmap(
                        self.ubase + page_index * arch::PGSIZE,
                        frame_get_page(frame),
                        perm
                    );
                    Frame::Cow(frame.clone())
                }
            }
        }).collect();

        let mut self_pagetable = self_pagetable.write();
        self.frames.iter_mut().enumerate().for_each(|(page_index, frame)| {
            *frame = match frame {
                Frame::Unallocated => Frame::Unallocated,
                Frame::Allocated(frame) | Frame::Cow(frame) => {
                    self_pagetable.mmap_replace(
                        self.ubase + page_index * arch::PGSIZE,
                        frame_get_page(frame),
                        MapPerm::R | MapPerm::U
                    );
                    Frame::Cow(frame.clone())
                }
            }
        });

        // let frames = self.fork_pages(self_pagetable, new_pagetable, index_to_uaddr);

        let new_area = AnonymousArea {
            ubase: self.ubase,
            perm: self.perm,
            frames,
        };

        Box::new(new_area)
    }

    fn try_to_fix_memory_fault(&mut self, uaddr: usize, access_type: MemAccessType, addrspace: &Arc<AddrSpace>) -> bool {
        debug_assert!(uaddr >= self.ubase);

        if access_type == MemAccessType::Read && !self.perm.contains(MapPerm::R) {
            return false;
        }
        if access_type == MemAccessType::Write && !self.perm.contains(MapPerm::W) {
            return false;
        }
        if access_type == MemAccessType::Execute && !self.perm.contains(MapPerm::X) {
            return false;
        }

        let page_index = (uaddr - self.ubase) / arch::PGSIZE;
        if page_index < self.frames.len() {
            match &self.frames[page_index] {
                Frame::Unallocated => {
                    // ktrace!("Fixing memory fault by allocating page at address: {:#x}, page index: {}", uaddr, page_index);
                    self.allocate_page(page_index, addrspace);
                }
                Frame::Allocated(frame) => {
                    // Page is already allocated, this shouldn't happen
                    // ktrace!("Memory fault on already allocated page at address: {:#x}", uaddr);
                    #[cfg(feature = "swap-memory")]
                    self.handle_memory_fault_on_swap(frame, addrspace);
                    
                    #[cfg(not(feature = "swap-memory"))]
                    panic!("Memory fault on already allocated page at address: {:#x}, access_type: {:?}, perm: {:?}", uaddr, access_type, self.perm);
                }
                Frame::Cow(_) => {
                    if access_type == MemAccessType::Write {
                        self.copy_on_write_page(page_index, addrspace);
                    } else {
                        // ktrace!("Memory fault on CoW page for read access at address: {:#x}", uaddr);
                        panic!("Read access fault on CoW page at address: {:#x}", uaddr);
                    }
                }
            }
            
            true
        } else {
            false
        }
    }

    fn set_ubase(&mut self, ubase: usize) {
        assert!(ubase % arch::PGSIZE == 0, "ubase should be page-aligned");
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
        };

        (
            self, 
            Box::new(new_area)
        )
    }

    fn ubase(&self) -> usize {
        self.ubase
    }

    fn alloc_new_page(&mut self, _page_index: usize, _pagetable: &RwLock<PageTable>) -> PhysPageFrame {
        PhysPageFrame::alloc_zeroed()
    }

    fn alloc_for_cow_page(&mut self, _page_index: usize, old_frame: Arc<PhysPageFrame>) -> PhysPageFrame {
        old_frame.copy()
    }

    fn perm(&self) -> MapPerm {
        self.perm
    }

    fn set_perm(&mut self, perm: MapPerm, pagetable: &RwLock<PageTable>) {
        self.perm = perm;

        // kinfo!("Setting permissions for AnonymousArea at ubase {:#x} to {:?}", self.ubase, perm);

        let mut pagetable = pagetable.write();
        for (page_index, frame) in self.frames.iter().enumerate() {
            if let Frame::Allocated(frame) = frame {
                let vaddr = self.ubase + page_index * arch::PGSIZE;
                let paddr = frame_get_page(frame);
                pagetable.mmap_replace(vaddr, paddr, perm);
                // kinfo!("Updated page table permissions for anonymous page at {:#x} to {:?}", vaddr, perm);
            }
            // Note: We don't update COW pages here as they should maintain
            // their current permission state until the next write access
        }
    }

    fn unmap(&mut self, pagetable: &RwLock<PageTable>) {
        let mut pagetable = pagetable.write();
        for (page_index, frame) in self.frames.iter_mut().enumerate() {
            #[cfg(feature = "swap-memory")]
            if let Frame::Allocated(frame) | Frame::Cow(frame) = frame {
                if frame.is_swapped_out() {
                    continue;
                } else {
                    pagetable.munmap(frame.uaddr());
                }
            }
            #[cfg(not(feature = "swap-memory"))]
            if let Frame::Allocated(_) | Frame::Cow(_) = frame {
                let uaddr = self.ubase + page_index * arch::PGSIZE;
                pagetable.munmap(uaddr);
            }
            *frame = Frame::Unallocated;
        }
    }
}
