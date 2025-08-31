use alloc::sync::Arc;
use alloc::vec::Vec;
use alloc::boxed::Box;
use spin::RwLock;

use crate::arch;
use crate::arch::{PageTable, PageTableTrait};
use crate::kernel::mm::frame::PhysPageFrame;
use crate::kernel::mm::maparea::area::Area;
use crate::kernel::mm::{MapPerm, MemAccessType};
use crate::fs::file::File;

use super::area::Frame;

pub struct ELFArea {
    ubase: usize,
    perm: MapPerm,
    
    file: Arc<File>,
    file_offset: usize,
    
    file_length: usize,
    memory_size: usize,
    frames: Vec<Frame>,
}

impl ELFArea {
    pub fn new(
        ubase: usize,
        perm: MapPerm,
        file: Arc<File>, 
        file_offset: usize, 
        file_length: usize, 
        memory_size: usize
    ) -> Self {
        // We only handle cases where file_offset and ubase are page-aligned.
        // The alignment should be guaranteed by the caller.
        assert!(ubase % arch::PGSIZE == 0, "ubase should be page-aligned");
        assert!(file_offset % arch::PGSIZE == 0, "file_offset should be page-aligned");

        let page_count = (memory_size + arch::PGSIZE - 1) / arch::PGSIZE;
        let frames = Vec::from_iter((0..page_count).map(|_| Frame::Unallocated));
        Self {
            ubase,
            perm,
            file,
            file_offset,
            file_length,
            memory_size,
            frames,
        }
    }

    fn load_page(&mut self, page_index: usize, pagetable: &RwLock<PageTable>) -> usize {
        assert!(page_index < self.frames.len());
        assert!(self.frames[page_index].is_unallocated());

        let area_offset = page_index * arch::PGSIZE;
        let file_offset = self.file_offset + area_offset;
        
        let frame = PhysPageFrame::new_zeroed();
        if area_offset < self.file_length {
            let mut buffer = [0u8; arch::PGSIZE];
            // Read up to a page, but not beyond the file length for this segment.
            let length = core::cmp::min(self.file_length - area_offset, arch::PGSIZE);
            self.file.read_at(&mut buffer[..length], file_offset).expect("Failed to read file");
            frame.copy_from_slice(0, &buffer[..length]);
        }

        let page = frame.get_page();

        pagetable.write().mmap(self.ubase + area_offset, frame.get_page(), self.perm);
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
                Err(cow) => cow.copy(),
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

impl Area for ELFArea {
    fn translate_read(&mut self, uaddr: usize, pagetable: &RwLock<PageTable>) -> Option<usize> {
        assert!(uaddr >= self.ubase);

        let page_index = (uaddr - self.ubase) / arch::PGSIZE;
        let page_offset = (uaddr - self.ubase) % arch::PGSIZE;
        
        if let Some(page_frame) = self.frames.get(page_index) {
            let page = match page_frame {
                Frame::Unallocated => {
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

        let page_index = (uaddr - self.ubase) / arch::PGSIZE;
        let page_offset = (uaddr - self.ubase) % arch::PGSIZE;
        
        if let Some(page_frame) = self.frames.get(page_index) {
            let page = match page_frame {
                Frame::Unallocated => {
                    self.load_page(page_index, pagetable)
                }
                Frame::Allocated(frame) => {
                    frame.get_page()
                }
                Frame::Cow(_) => {
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
                        perm
                    );
                    Frame::Cow(frame.clone())
                }
            };
        });

        let new_area = ELFArea {
            ubase: self.ubase,
            perm: self.perm,
            file: self.file.clone(),
            file_offset: self.file_offset,
            file_length: self.file_length,
            memory_size: self.memory_size,
            frames,
        };

        Box::new(new_area)
    }

    fn try_to_fix_memory_fault(&mut self, uaddr: usize, access_type: MemAccessType, pagetable: &RwLock<PageTable>) -> bool {
        assert!(uaddr >= self.ubase);

        if access_type == MemAccessType::Execute && !self.perm.contains(MapPerm::X) {
            return false;
        }
        if access_type == MemAccessType::Write && !self.perm.contains(MapPerm::W) {
            return false;
        }

        let page_index = (uaddr - self.ubase) / arch::PGSIZE;
        if page_index < self.frames.len() {
            match &self.frames[page_index] {
                Frame::Unallocated => {
                    self.load_page(page_index, pagetable);
                }
                Frame::Allocated(_) => {
                    panic!("Page is already allocated.");
                }
                Frame::Cow(_) => {
                    if access_type == MemAccessType::Write {
                        self.copy_on_write_page(page_index, pagetable);
                    } else {
                        panic!("Page is already allocated for read and execute access.");
                    }
                }
            }
            true
        } else {
            false
        }
    }

    fn page_count(&self) -> usize {
        self.frames.len()
    }

    fn split(&mut self, uaddr: usize) -> Box<dyn Area> {
        assert!(uaddr % arch::PGSIZE == 0, "Split address must be page-aligned");
        assert!(uaddr > self.ubase, "Split address must be greater than ubase");
        assert!(uaddr < self.ubase + self.size(), "Split address out of bounds");

        let split_index = (uaddr - self.ubase) / arch::PGSIZE;
        let split_offset = split_index * arch::PGSIZE;
        let remaining_frames = self.frames.split_off(split_index);

        // Calculate the new file_length and memory_size for the second area
        let new_file_length = if self.file_length > split_offset {
            self.file_length - split_offset
        } else {
            0
        };
        
        let new_memory_size = self.memory_size - split_offset;

        // Update the file_length and memory_size for the first area (self)
        self.file_length = core::cmp::min(self.file_length, split_offset);
        self.memory_size = split_offset;

        let new_area = ELFArea {
            ubase: uaddr,
            perm: self.perm,
            file: self.file.clone(),
            file_offset: self.file_offset + split_offset,
            file_length: new_file_length,
            memory_size: new_memory_size,
            frames: remaining_frames,
        };

        Box::new(new_area)
    }

    fn set_perm(&mut self, perm: MapPerm, _pagetable: &RwLock<PageTable>) {
        self.perm = perm;
        // Note: This method only updates the internal permission field.
        // The actual page table permissions are not updated here.
        // Page table permissions will be updated when the pages are accessed again
        // through translate_read/translate_write, or when a memory fault occurs.
        // 
        // For immediate page table update, the caller should use the page table's
        // mprotect method for each allocated page.
    }

    fn type_name(&self) -> &'static str {
        "ELFArea"
    }
}
