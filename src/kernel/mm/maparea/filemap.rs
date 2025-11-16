use alloc::sync::Arc;
use alloc::vec::Vec;
use alloc::boxed::Box;
use spin::RwLock;

use crate::{kdebug, ktrace, kwarn};
use crate::arch::{PageTable, PageTableTrait};
use crate::arch;
use crate::kernel::errno::Errno;
use crate::kernel::mm::frame::PhysPageFrame;
use crate::kernel::mm::maparea::area::Area;
use crate::kernel::mm::{MapPerm, MemAccessType};
use crate::fs::file::File;

enum Frame {
    Unallocated,
    Allocated(Arc<PhysPageFrame>),
    Cow(Arc<PhysPageFrame>),
}

impl Frame {
    pub fn is_unallocated(&self) -> bool {
        matches!(self, Frame::Unallocated)
    }
}

pub struct FileMapArea {
    ubase: usize,
    perm: MapPerm,
    
    file: Arc<File>,
    file_offset: usize,
    file_length: usize,
    
    frames: Vec<Frame>,
}

impl FileMapArea {
    pub fn new(
        ubase: usize,
        perm: MapPerm,
        file: Arc<File>, 
        file_offset: usize,
        file_length: usize
    ) -> Self {
        // File mapping areas should be page-aligned
        assert!(ubase % arch::PGSIZE == 0, "ubase should be page-aligned");
        assert!(file_offset % arch::PGSIZE == 0, "file_offset should be page-aligned");

        let page_count = (file_length + arch::PGSIZE - 1) / arch::PGSIZE;
        let frames = Vec::from_iter((0..page_count).map(|_| Frame::Unallocated));
        Self {
            ubase,
            perm,
            file,
            file_offset,
            file_length,
            frames,
        }
    }

    fn load_page(&mut self, page_index: usize, pagetable: &RwLock<PageTable>) -> usize {
        assert!(page_index < self.frames.len());
        assert!(self.frames[page_index].is_unallocated());

        let area_offset = page_index * arch::PGSIZE;
        let file_offset = self.file_offset + area_offset;
        
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
                    // ktrace!("Loaded page {} from file at offset {:#x}, length: {}", page_index, file_offset, length);
                }
                Err(_) => {
                    kwarn!("Failed to read from file at offset {:#x}", file_offset);
                    // Keep the page zeroed if file read fails
                }
            }
        }

        let page = frame.get_page();

        pagetable.write().mmap(self.ubase + area_offset, page, self.perm);
        self.frames[page_index] = Frame::Allocated(Arc::new(frame));

        page
    }

    fn copy_on_write_page(&mut self, page_index: usize, pagetable: &RwLock<PageTable>) -> usize {
        assert!(page_index < self.frames.len());
        
        // Verify that the original mapping has write permission
        if !self.perm.contains(MapPerm::W) {
            panic!("Attempting copy-on-write on a non-writable mapping at page index {}", page_index);
        }

        let area_offset = page_index * arch::PGSIZE;
        let frame = match &self.frames[page_index] {
            Frame::Cow(frame) => frame.copy(),
            _ => panic!("Invalid type for copy-on-write"),
        };

        let page = frame.get_page();

        pagetable.write().mmap_replace(self.ubase + area_offset, page, self.perm);
        self.frames[page_index] = Frame::Allocated(Arc::new(frame));

        // ktrace!("Copy-on-write triggered for file mapping page {} at address {:#x}", page_index, self.ubase + area_offset);

        page
    }

    fn write_back_page(&self, page_index: usize) -> Result<(), crate::kernel::errno::Errno> {
        if let Frame::Allocated(frame) = &self.frames[page_index] {
            let area_offset = page_index * arch::PGSIZE;
            let file_offset = self.file_offset + area_offset;
            
            // Only write back if this page is within the file_length
            if area_offset < self.file_length {
                // Calculate how much data we should write back:
                // - Don't write beyond the file_length boundary
                // - Don't write beyond one page
                let length = core::cmp::min(self.file_length - area_offset, arch::PGSIZE);
                let buffer = unsafe {
                    core::slice::from_raw_parts(frame.get_page() as *const u8, length)
                };
                
                // For now, we use the inode's writeat method directly
                // In a real implementation, you might want to add write_at to File
                self.file.write_at(buffer, file_offset)?;
                ktrace!("Wrote back page {} to file at offset {:#x}, length: {}", page_index, file_offset, length);
            }
        }
        Ok(())
    }

    pub fn sync(&self) -> Result<(), Errno> {
        if !self.file.flags.writable {
            return Ok(());
        }
        
        for (page_index, frame) in self.frames.iter().enumerate() {
            if let Frame::Allocated(_) = frame {
                self.write_back_page(page_index)?;
            }
        }
        Ok(())
    }
}

impl Area for FileMapArea {
    fn translate_read(&mut self, uaddr: usize, pagetable: &RwLock<PageTable>) -> Option<usize> {
        assert!(uaddr >= self.ubase);

        let page_index = (uaddr - self.ubase) / arch::PGSIZE;
        let page_offset = (uaddr - self.ubase) % arch::PGSIZE;
        
        if let Some(page_frame) = self.frames.get(page_index) {
            let page = match page_frame {
                Frame::Unallocated => {
                    // Lazy loading: load page from file on first access
                    self.load_page(page_index, pagetable)
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
                    // Lazy loading: load page from file on first write
                    self.load_page(page_index, pagetable)
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
        let cow_perm = self.perm - MapPerm::W;
        
        let mut pagetable = new_pagetable.write();
        let frames = self.frames.iter().enumerate().map(|(page_index, frame)| {
            match frame {
                Frame::Unallocated => Frame::Unallocated,
                Frame::Allocated(frame) => {
                    pagetable.mmap(
                        self.ubase + page_index * arch::PGSIZE, 
                        frame.get_page(),
                        cow_perm
                    );
                    Frame::Cow(frame.clone())
                }
                Frame::Cow(frame) => {
                    pagetable.mmap(
                        self.ubase + page_index * arch::PGSIZE, 
                        frame.get_page(),
                        cow_perm
                    );
                    Frame::Cow(frame.clone())
                },
            }
        }).collect();

        if self.perm.contains(MapPerm::W) {
            // Update original mapping to be COW if it was writable
            let mut self_pagetable = self_pagetable.write();
            self.frames.iter_mut().enumerate().for_each(|(page_index, frame)| {
                if let Frame::Allocated(f) = frame {
                    self_pagetable.mmap_replace(
                        self.ubase + page_index * arch::PGSIZE,
                        f.get_page(),
                        cow_perm
                    );
                    *frame = Frame::Cow(f.clone());
                }
            });
        }

        let new_area = FileMapArea {
            ubase: self.ubase,
            perm: self.perm,
            file: self.file.clone(),
            file_offset: self.file_offset,
            file_length: self.file_length,
            frames,
        };

        Box::new(new_area)
    }

    fn try_to_fix_memory_fault(&mut self, uaddr: usize, access_type: MemAccessType, pagetable: &RwLock<PageTable>) -> bool {
        assert!(uaddr >= self.ubase);

        if !self.perm.contains(MapPerm::W) && access_type == MemAccessType::Write {
            return false;
        }

        // ktrace!("Fixing memory fault for file mapping at address: {:#x}", uaddr);

        let page_index = (uaddr - self.ubase) / arch::PGSIZE;
        if page_index < self.frames.len() {
            match &self.frames[page_index] {
                Frame::Unallocated => {
                    // ktrace!("Fixing memory fault by loading file page at address: {:#x}, page index: {}", uaddr, page_index);
                    self.load_page(page_index, pagetable);
                    // ktrace!("Memory fault fixed by loading page at address: {:#x}", uaddr);
                }
                Frame::Allocated(_) => {
                    // Page is already allocated, this shouldn't happen
                    panic!("Memory fault on already allocated file page at address: {:#x}", uaddr);
                }
                Frame::Cow(_) => {
                    assert!(access_type == MemAccessType::Write, "Memory fault on CoW file page for read access at address: {:#x}", uaddr);
                    self.copy_on_write_page(page_index, pagetable);
                }
            }
            
            true
        } else {
            kdebug!("Memory fault at address {:#x} for file mapping, but page index {} is out of bounds", uaddr, page_index);
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
        assert!(uaddr % arch::PGSIZE == 0, "Split address must be page-aligned");
        assert!(uaddr > self.ubase, "Split address must be greater than ubase");
        assert!(uaddr < self.ubase + self.size(), "Split address out of bounds");

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

        let new_area = FileMapArea {
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
            if let Frame::Allocated(frame) = frame {
                let vaddr = self.ubase + page_index * arch::PGSIZE;
                let paddr = frame.get_page();
                pagetable.mmap_replace(vaddr, paddr, perm);
                ktrace!("Updated page table permissions for file mapping page at {:#x} to {:?}", vaddr, perm);
            }
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
        "FileMapArea"
    }
}

impl Drop for FileMapArea {
    fn drop(&mut self) {
        // Write back all dirty pages when the mapping is dropped
        if let Err(_) = self.sync() {
            ktrace!("Failed to sync file mapping during drop");
        }
    }
}
