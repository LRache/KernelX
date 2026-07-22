use alloc::boxed::Box;
use alloc::sync::Arc;
use alloc::vec::Vec;

use crate::arch;
use crate::arch::{PageTable, PageTableTrait};
use crate::fs::file::{FileOps, RandomAccessFile};
use crate::kernel::mm::maparea::{
    Area, MapAreaInfo, MapChange, MapChangeEvent, MapChangeNotifier, MemoryFaultSignal, PinPageFrame,
};
use crate::kernel::mm::{AddrSpace, MapPerm, MemAccessType, PhysPageFrame};
use crate::klib::SpinLock;

use super::area::Frame;

pub struct ELFArea {
    ubase: usize,
    perm: MapPerm,

    file: Arc<RandomAccessFile>,
    file_offset: usize,

    file_length: usize,
    memory_size: usize,
    frames: Vec<Frame>,
}

impl ELFArea {
    pub fn new(
        ubase: usize,
        perm: MapPerm,
        file: Arc<RandomAccessFile>,
        file_offset: usize,
        file_length: usize,
        memory_size: usize,
    ) -> Self {
        // We only handle cases where file_offset and ubase are page-aligned.
        // The alignment should be guaranteed by the caller.
        debug_assert!(ubase % arch::PGSIZE == 0, "ubase should be page-aligned");
        debug_assert!(file_offset % arch::PGSIZE == 0, "file_offset should be page-aligned");

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

    fn load_page(&mut self, page_index: usize, pagetable: &SpinLock<PageTable>) -> usize {
        debug_assert!(page_index < self.frames.len());
        debug_assert!(self.frames[page_index].is_unallocated());

        let area_offset = page_index * arch::PGSIZE;
        let file_offset = self.file_offset + area_offset;

        let frame = Arc::new(PhysPageFrame::alloc_zeroed());
        if area_offset < self.file_length {
            // Read up to a page, but not beyond the file length for this segment.
            let length = core::cmp::min(self.file_length - area_offset, arch::PGSIZE);
            self.file
                .pread(&mut frame.slice()[..length], file_offset)
                .expect("Failed to read file");
        }

        let page = frame.get_page();

        pagetable.lock().mmap(self.ubase + area_offset, &frame, self.perm);
        self.frames[page_index] = Frame::Allocated(frame);

        page
    }

    fn copy_on_write_page(
        &mut self,
        page_index: usize,
        addrspace: &AddrSpace,
        map_change_notifier: &MapChangeNotifier<'_>,
    ) {
        assert!(page_index < self.frames.len());
        assert!(self.frames[page_index].is_cow());

        let uaddr = self.ubase + page_index * arch::PGSIZE;
        map_change_notifier.before_map_change(MapChange {
            uaddr,
            page_count: 1,
            event: MapChangeEvent::Remap,
        });

        let cow_frame = core::mem::replace(&mut self.frames[page_index], Frame::Unallocated);
        let new_frame = if let Frame::Cow(frame) = cow_frame {
            match Arc::try_unwrap(frame) {
                Ok(only) => only,
                Err(cow) => cow.copy(),
            }
        } else {
            unreachable!();
        };

        let new_frame = Arc::new(new_frame);

        addrspace.pagetable().lock().mmap_replace(uaddr, &new_frame, self.perm);
        self.frames[page_index] = Frame::Allocated(new_frame);
    }

    fn map_cow_page(&self, page_index: usize, frame: &Arc<PhysPageFrame>, addrspace: &AddrSpace) -> usize {
        let page = frame.get_page();
        let uaddr = self.ubase + page_index * arch::PGSIZE;

        addrspace
            .pagetable()
            .lock()
            .mmap_replace(uaddr, frame, self.perm - MapPerm::W);

        page
    }
}

impl Area for ELFArea {
    fn translate_read(&mut self, uaddr: usize, addrspace: &AddrSpace) -> Option<PinPageFrame> {
        assert!(uaddr >= self.ubase);

        let page_index = (uaddr - self.ubase) / arch::PGSIZE;
        if page_index >= self.frames.len() {
            return None;
        }
        if self.frames[page_index].is_unallocated() {
            self.load_page(page_index, addrspace.pagetable());
        }
        match &self.frames[page_index] {
            Frame::Allocated(frame) | Frame::Cow(frame) => Some(PinPageFrame::stable(frame.clone())),
            Frame::Unallocated => unreachable!(),
        }
    }

    fn translate_write(
        &mut self,
        uaddr: usize,
        addrspace: &AddrSpace,
        map_change_notifier: &MapChangeNotifier<'_>,
    ) -> Option<PinPageFrame> {
        assert!(uaddr >= self.ubase);

        let page_index = (uaddr - self.ubase) / arch::PGSIZE;

        if page_index >= self.frames.len() {
            return None;
        }

        if self.frames[page_index].is_unallocated() {
            self.load_page(page_index, addrspace.pagetable());
        } else {
            if self.frames[page_index].is_cow() {
                self.copy_on_write_page(page_index, addrspace, map_change_notifier);
            }
        }

        let Frame::Allocated(frame) = &self.frames[page_index] else {
            unreachable!();
        };
        Some(PinPageFrame::stable(frame.clone()))
    }

    fn get_frame(
        &mut self,
        uaddr: usize,
        addrspace: &AddrSpace,
        map_change_notifier: &MapChangeNotifier<'_>,
    ) -> Option<PinPageFrame> {
        self.translate_write(uaddr, addrspace, map_change_notifier)
    }

    fn perm(&self) -> MapPerm {
        self.perm
    }

    fn fork(
        &mut self,
        self_pagetable: &SpinLock<PageTable>,
        tlb_changed: &mut bool,
        _addrspace: &Arc<AddrSpace>,
    ) -> Box<dyn Area> {
        let cow_perm = self.perm - MapPerm::W;
        let frames = self
            .frames
            .iter()
            .map(|frame| match frame {
                Frame::Unallocated => Frame::Unallocated,
                Frame::Allocated(frame) | Frame::Cow(frame) => Frame::Cow(frame.clone()),
            })
            .collect();

        self.frames.iter_mut().enumerate().for_each(|(page_index, frame)| {
            *frame = match frame {
                Frame::Unallocated => Frame::Unallocated,
                Frame::Allocated(frame) | Frame::Cow(frame) => {
                    let uaddr = self.ubase + page_index * arch::PGSIZE;
                    *tlb_changed |= self_pagetable.lock().mmap_replace_perm_no_flush(uaddr, cow_perm);
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

    fn try_to_fix_memory_fault(
        &mut self,
        uaddr: usize,
        access_type: MemAccessType,
        addrspace: &AddrSpace,
        map_change_notifier: &MapChangeNotifier<'_>,
    ) -> Result<(), MemoryFaultSignal> {
        assert!(uaddr >= self.ubase);

        if access_type == MemAccessType::Execute && !self.perm.contains(MapPerm::X) {
            return Err(MemoryFaultSignal::Segv);
        }
        if access_type == MemAccessType::Write && !self.perm.contains(MapPerm::W) {
            return Err(MemoryFaultSignal::Segv);
        }

        let page_index = (uaddr - self.ubase) / arch::PGSIZE;
        if page_index < self.frames.len() {
            match &self.frames[page_index] {
                Frame::Unallocated => {
                    self.load_page(page_index, addrspace.pagetable());
                }
                Frame::Allocated(_) => {
                    // Maybe the page is allocated and mapped by the other task.
                }
                Frame::Cow(frame) => {
                    if access_type == MemAccessType::Write {
                        self.copy_on_write_page(page_index, addrspace, map_change_notifier);
                    } else {
                        self.map_cow_page(page_index, frame, addrspace);
                    }
                }
            }
            Ok(())
        } else {
            Err(MemoryFaultSignal::Segv)
        }
    }

    fn page_count(&self) -> usize {
        self.frames.len()
    }

    fn ubase(&self) -> usize {
        self.ubase
    }

    fn split(mut self: Box<Self>, uaddr: usize) -> (Box<dyn Area>, Box<dyn Area>) {
        debug_assert!(uaddr % arch::PGSIZE == 0, "Split address must be page-aligned");
        debug_assert!(
            uaddr >= self.ubase && uaddr < self.ubase + self.size(),
            "Split address must be within area bounds"
        );

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

        (self, Box::new(new_area))
    }

    fn unmap(&mut self, pagetable: &SpinLock<PageTable>) {
        let mut tlb_changed = false;
        for (page_index, frame) in self.frames.iter().enumerate() {
            if let Frame::Allocated(frame) | Frame::Cow(frame) = frame {
                // The page may not be mapped to the page table if it was loaded by
                // `translate_read` or `translate_write` but never accessed afterwards.
                tlb_changed |= pagetable
                    .lock()
                    .munmap_with_check_no_flush(self.ubase + page_index * arch::PGSIZE, frame.get_page());
            }
        }
        if tlb_changed {
            arch::flush_tlb_all();
        }
        for frame in &mut self.frames {
            *frame = Frame::Unallocated;
        }
    }

    fn set_perm(&mut self, perm: MapPerm, pagetable: &SpinLock<PageTable>, tlb_changed: &mut bool) {
        self.perm = perm;
        let cow_perm = perm - MapPerm::W;
        self.frames.iter().enumerate().for_each(|(page_index, frame)| {
            let uaddr = self.ubase + page_index * arch::PGSIZE;
            match frame {
                Frame::Allocated(_) => {
                    *tlb_changed |= pagetable.lock().mmap_replace_perm_no_flush(uaddr, perm);
                }
                Frame::Cow(_) => {
                    *tlb_changed |= pagetable.lock().mmap_replace_perm_no_flush(uaddr, cow_perm);
                }
                Frame::Unallocated => {}
            }
        });
    }

    fn type_name(&self) -> &'static str {
        "elf"
    }

    fn map_area_info(&self) -> MapAreaInfo {
        let mut info = MapAreaInfo::new(self.ubase(), self.ubase() + self.size(), self.perm);
        info.offset = self.file_offset;
        (info.dev_major, info.dev_minor) = self.file.get_dentry().map(|dentry| (0, dentry.sno())).unwrap_or((0, 0));
        info.inode = self
            .file
            .get_dentry()
            .map(|dentry| dentry.ino() as u64)
            .or_else(|| self.file.get_inode().map(|inode| inode.get_ino() as u64))
            .unwrap_or(0);
        info.path = self.file.get_dentry().map(|dentry| dentry.get_path());
        info
    }
}
