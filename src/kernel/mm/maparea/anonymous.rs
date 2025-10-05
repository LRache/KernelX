use alloc::sync::Arc;
use alloc::vec::Vec;
use alloc::boxed::Box;
use spin::RwLock;

use crate::{arch, kinfo, ktrace};
use crate::arch::{PageTable, PageTableTrait};
use crate::kernel::mm::frame::PhysPageFrame;
use crate::kernel::mm::maparea::area::Area;
use crate::kernel::mm::{MapPerm, MemAccessType};

use super::area::Frame;

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

    fn allocate_page(&mut self, page_index: usize, pagetable: &RwLock<PageTable>) -> usize {
        assert!(page_index < self.frames.len());
        assert!(self.frames[page_index].is_unallocated());

        let area_offset = page_index * arch::PGSIZE;
        
        // Create a new zeroed page for anonymous memory
        let frame = PhysPageFrame::new_zeroed();
        let page = frame.get_page();

        pagetable.write().mmap(self.ubase + area_offset, page, self.perm);
        self.frames[page_index] = Frame::Allocated(Arc::new(frame));

        page
    }

    fn copy_on_write_page(&mut self, page_index: usize, pagetable: &RwLock<PageTable>) -> usize {
        assert!(page_index < self.frames.len());
        assert!(self.frames[page_index].is_cow());

        let cow_frame = core::mem::replace(&mut self.frames[page_index], Frame::Unallocated);
        let new_frame = if let Frame::Cow(frame) = cow_frame {
            match Arc::try_unwrap(frame) {
                Ok(only) => only,
                Err(cow) => cow.copy()
            }
        } else {
            unreachable!();
        };

        let page = new_frame.get_page();

        pagetable.write().mmap_replace(self.ubase + page_index * arch::PGSIZE, page, self.perm);
        self.frames[page_index] = Frame::Allocated(Arc::new(new_frame));

        page
    }
}

impl Area for AnonymousArea {
    fn translate_read(&mut self, uaddr: usize, pagetable: &RwLock<PageTable>) -> Option<usize> {
        assert!(uaddr >= self.ubase);

        let page_index = (uaddr - self.ubase) / arch::PGSIZE;
        let page_offset = (uaddr - self.ubase) % arch::PGSIZE;
        
        if let Some(page_frame) = self.frames.get(page_index) {
            let page = match page_frame {
                Frame::Unallocated => {
                    // Lazy allocation: allocate page on first access
                    self.allocate_page(page_index, pagetable)
                }
                Frame::Allocated(frame) => {
                    frame.get_page()
                }
                Frame::Cow(frame) => {
                    frame.get_page()
                }
            };

            Some(page + page_offset)
        } else {
            None
        }
    }

    fn translate_write(&mut self, uaddr: usize, pagetable: &RwLock<PageTable>) -> Option<usize> {
        assert!(uaddr >= self.ubase);

        if !self.perm.contains(MapPerm::W) {
            return None;
        }

        let page_index = (uaddr - self.ubase) / arch::PGSIZE;
        let page_offset = (uaddr - self.ubase) % arch::PGSIZE;
        
        if let Some(page_frame) = self.frames.get_mut(page_index) {
            let page = match page_frame {
                Frame::Unallocated => {
                    self.allocate_page(page_index, pagetable)
                }
                Frame::Allocated(frame) => {
                    frame.get_page()
                }
                Frame::Cow(_) => {
                    // Copy-on-write: create a new copy for this process
                    self.copy_on_write_page(page_index, pagetable)
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
                        frame.get_page(),
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
                        frame.get_page(),
                        MapPerm::R | MapPerm::U
                    );
                    Frame::Cow(frame.clone())
                }
            }
        });

        let new_area = AnonymousArea {
            ubase: self.ubase,
            perm: self.perm,
            frames,
        };

        Box::new(new_area)
    }

    fn try_to_fix_memory_fault(&mut self, uaddr: usize, access_type: MemAccessType, pagetable: &RwLock<PageTable>) -> bool {
        assert!(uaddr >= self.ubase);

        let page_index = (uaddr - self.ubase) / arch::PGSIZE;
        if page_index < self.frames.len() {
            match &self.frames[page_index] {
                Frame::Unallocated => {
                    ktrace!("Fixing memory fault by allocating page at address: {:#x}, page index: {}", uaddr, page_index);
                    self.allocate_page(page_index, pagetable);
                }
                Frame::Allocated(_) => {
                    // Page is already allocated, this shouldn't happen
                    ktrace!("Memory fault on already allocated page at address: {:#x}", uaddr);
                    return false;
                }
                Frame::Cow(_) => {
                    if access_type == MemAccessType::Write {
                        self.copy_on_write_page(page_index, pagetable);
                    } else {
                        ktrace!("Memory fault on CoW page for read access at address: {:#x}", uaddr);
                        return false;
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
        assert!(uaddr % arch::PGSIZE == 0, "uaddr should be page-aligned");
        assert!(uaddr > self.ubase && uaddr < self.ubase + self.size(), "uaddr out of range for split");

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

    fn set_perm(&mut self, perm: MapPerm, pagetable: &RwLock<PageTable>) {
        self.perm = perm;

        kinfo!("Setting permissions for AnonymousArea at {:#x} to {:?}", self.ubase, perm);

        let mut pagetable = pagetable.write();
        for (page_index, frame) in self.frames.iter().enumerate() {
            if let Frame::Allocated(frame) = frame {
                let vaddr = self.ubase + page_index * arch::PGSIZE;
                let paddr = frame.get_page();
                pagetable.mmap_replace(vaddr, paddr, perm);
                kinfo!("Updated page table permissions for anonymous page at {:#x} to {:?}", vaddr, perm);
            }
            // Note: We don't update COW pages here as they should maintain
            // their current permission state until the next write access
        }
    }

    fn unmap(&mut self, pagetable: &RwLock<PageTable>) {
        let mut pagetable = pagetable.write();
        for (page_index, frame) in self.frames.iter_mut().enumerate() {
            if let Frame::Allocated(_) | Frame::Cow(_) = frame {
                let uaddr = self.ubase + page_index * arch::PGSIZE;
                pagetable.munmap(uaddr);
            }
            *frame = Frame::Unallocated;
        }
    }

    fn type_name(&self) -> &'static str {
        "AnonymousArea"
    }
}
