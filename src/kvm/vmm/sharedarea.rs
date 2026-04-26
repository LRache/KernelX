use alloc::boxed::Box;
use alloc::collections::BTreeSet;

use crate::arch::{self, PageTable, PageTableTrait};
use crate::kernel::mm::maparea::Area;
use crate::kernel::mm::{AddrSpace, MapPerm, MemAccessType};
use crate::klib::SpinLock;

use super::vmaparea::SharedFrames;

pub struct KVMSharedArea {
    ubase: usize,
    perm: MapPerm,
    frames: SharedFrames,
    mapped_pages: BTreeSet<usize>,
}

impl KVMSharedArea {
    pub fn new(ubase: usize, perm: MapPerm, frames: SharedFrames) -> Self {
        debug_assert!(ubase % arch::PGSIZE == 0, "ubase should be page-aligned");

        Self {
            ubase,
            perm,
            frames,
            mapped_pages: BTreeSet::new(),
        }
    }

    fn page_index_and_offset(&self, uaddr: usize) -> Option<(usize, usize)> {
        if uaddr < self.ubase {
            return None;
        }

        let offset = uaddr - self.ubase;
        let page_index = offset / arch::PGSIZE;
        if page_index >= self.frames.len() {
            return None;
        }

        Some((page_index, offset % arch::PGSIZE))
    }

    fn map_page(&mut self, page_index: usize, kpage: usize, pagetable: &SpinLock<PageTable>) {
        let uaddr = self.ubase + page_index * arch::PGSIZE;
        let mut pagetable = pagetable.lock();
        if self.mapped_pages.insert(page_index) {
            pagetable.mmap(uaddr, kpage, self.perm);
        } else {
            pagetable.mmap_replace(uaddr, kpage, self.perm);
        }
    }
}

impl Area for KVMSharedArea {
    fn translate_read(&mut self, uaddr: usize, addrspace: &AddrSpace) -> Option<usize> {
        let _ = addrspace;
        let (page_index, page_offset) = self.page_index_and_offset(uaddr)?;
        let kpage = self.frames.translate(page_index);

        Some(kpage + page_offset)
    }

    fn translate_write(&mut self, uaddr: usize, addrspace: &AddrSpace) -> Option<usize> {
        if !self.perm.contains(MapPerm::W) {
            return None;
        }
        self.translate_read(uaddr, addrspace)
    }

    fn ubase(&self) -> usize {
        self.ubase
    }

    fn set_ubase(&mut self, ubase: usize) {
        debug_assert!(ubase % arch::PGSIZE == 0, "ubase should be page-aligned");
        self.ubase = ubase;
    }

    fn perm(&self) -> MapPerm {
        self.perm
    }

    fn fork(&mut self, _self_pagetable: &SpinLock<PageTable>, new_pagetable: &mut PageTable) -> Box<dyn Area> {
        let mut mapped_pages = BTreeSet::new();
        for &page_index in &self.mapped_pages {
            if let Some(kpage) = self.frames.get_page(page_index) {
                new_pagetable.mmap(self.ubase + page_index * arch::PGSIZE, kpage, self.perm);
                mapped_pages.insert(page_index);
            }
        }

        Box::new(Self {
            ubase: self.ubase,
            perm: self.perm,
            frames: self.frames.clone(),
            mapped_pages,
        })
    }

    fn try_to_fix_memory_fault(
        &mut self,
        uaddr: usize,
        _access_type: MemAccessType,
        addrspace: &AddrSpace,
    ) -> Option<usize> {
        let (page_index, page_offset) = self.page_index_and_offset(uaddr)?;
        let kpage = self.frames.translate(page_index);

        self.map_page(page_index, kpage, addrspace.pagetable());
        Some(kpage + page_offset)
    }

    fn page_count(&self) -> usize {
        self.frames.len()
    }

    fn split(self: Box<Self>, uaddr: usize) -> (Box<dyn Area>, Box<dyn Area>) {
        debug_assert!(uaddr % arch::PGSIZE == 0, "uaddr should be page-aligned");
        debug_assert!(
            uaddr >= self.ubase && uaddr < self.ubase + self.size(),
            "uaddr out of range for split"
        );

        let Self {
            ubase,
            perm,
            frames,
            mapped_pages,
        } = *self;
        let split_index = (uaddr - ubase) / arch::PGSIZE;
        let (left_frames, right_frames) = frames.split_at(split_index);

        let mut left_mapped_pages = BTreeSet::new();
        let mut right_mapped_pages = BTreeSet::new();
        for page_index in mapped_pages {
            if page_index < split_index {
                left_mapped_pages.insert(page_index);
            } else {
                right_mapped_pages.insert(page_index - split_index);
            }
        }

        (
            Box::new(Self {
                ubase,
                perm,
                frames: left_frames,
                mapped_pages: left_mapped_pages,
            }),
            Box::new(Self {
                ubase: uaddr,
                perm,
                frames: right_frames,
                mapped_pages: right_mapped_pages,
            }),
        )
    }

    fn set_perm(&mut self, perm: MapPerm, pagetable: &SpinLock<PageTable>) {
        if perm == self.perm {
            return;
        }

        self.perm = perm;

        let mut pagetable = pagetable.lock();
        let mapped_pages = core::mem::take(&mut self.mapped_pages);
        for page_index in mapped_pages {
            let Some(kpage) = self.frames.get_page(page_index) else {
                continue;
            };

            let uaddr = self.ubase + page_index * arch::PGSIZE;
            if pagetable.munmap_with_check(uaddr, kpage) {
                pagetable.mmap(uaddr, kpage, perm);
                self.mapped_pages.insert(page_index);
            }
        }
    }

    fn unmap(&mut self, pagetable: &SpinLock<PageTable>) {
        let mut pagetable = pagetable.lock();
        let mapped_pages = core::mem::take(&mut self.mapped_pages);
        for page_index in mapped_pages {
            let Some(kpage) = self.frames.get_page(page_index) else {
                continue;
            };

            let uaddr = self.ubase + page_index * arch::PGSIZE;
            let _ = pagetable.munmap_with_check(uaddr, kpage);
        }
    }

    fn type_name(&self) -> &'static str {
        "kvm-user-shared"
    }
}
