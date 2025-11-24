use alloc::boxed::Box;
use alloc::sync::Arc;
use alloc::vec::Vec;
use spin::RwLock;

use crate::arch::{self, PageTable, PageTableTrait};
use crate::kernel::mm::PhysPageFrame;
use crate::kernel::mm::{MapPerm, MemAccessType};

#[derive(Debug)]
pub enum Frame {
    Unallocated,
    Allocated(Arc<PhysPageFrame>),
    Cow(Arc<PhysPageFrame>),
}

impl Frame {
    pub fn is_unallocated(&self) -> bool {
        matches!(self, Frame::Unallocated)
    }

    pub fn is_cow(&self) -> bool {
        matches!(self, Frame::Cow(_))
    }
}

pub trait Area {
    fn translate_read(&mut self, uaddr: usize, pagetable: &RwLock<PageTable>) -> Option<usize>;
    fn translate_write(&mut self, uaddr: usize, pagetable: &RwLock<PageTable>) -> Option<usize>;

    fn page_frames(&mut self) -> &mut [Frame] {
        unimplemented!();
    }

    fn ubase(&self) -> usize;

    fn set_ubase(&mut self, _ubase: usize) {
        unimplemented!(
            "set_ubase not implemented for the area type: {}",
            self.type_name()
        );
    }
    fn perm(&self) -> MapPerm {
        unimplemented!();
    }

    fn alloc_new_page(
        &mut self,
        _page_index: usize,
        _pagetable: &RwLock<PageTable>,
    ) -> PhysPageFrame {
        unimplemented!();
    }
    fn alloc_for_cow_page(
        &mut self,
        _page_index: usize,
        _old_frame: Arc<PhysPageFrame>,
    ) -> PhysPageFrame {
        unimplemented!();
    }

    fn fork(
        &mut self,
        self_pagetable: &RwLock<PageTable>,
        fork_pagetable: &RwLock<PageTable>,
    ) -> Box<dyn Area>;

    fn fork_pages(
        &mut self,
        self_pagetable: &RwLock<PageTable>,
        new_pagetable: &RwLock<PageTable>,
        index_to_uaddr: fn(usize, usize) -> usize,
    ) -> Vec<Frame> {
        let cow_perm = self.perm() - MapPerm::W;
        let writable = self.perm().contains(MapPerm::W);
        let ubase = self.ubase();

        let page_frames = self.page_frames();

        let mut self_pagetable = self_pagetable.write();
        let mut new_pagetable = new_pagetable.write();
        let forked_pages_frames = page_frames
            .iter_mut()
            .enumerate()
            .map(|(index, frame)| {
                let self_frame = core::mem::replace(frame, Frame::Unallocated);
                match self_frame {
                    Frame::Unallocated => Frame::Unallocated,
                    Frame::Allocated(self_frame) => {
                        *frame = Frame::Cow(self_frame.clone());
                        if writable {
                            self_pagetable.mmap_replace(
                                index_to_uaddr(ubase, index),
                                self_frame.get_page(),
                                cow_perm,
                            );
                        }
                        new_pagetable.mmap(index * arch::PGSIZE, self_frame.get_page(), cow_perm);
                        Frame::Cow(self_frame)
                    }
                    Frame::Cow(self_frame) => {
                        new_pagetable.mmap(
                            index_to_uaddr(ubase, index),
                            self_frame.get_page(),
                            cow_perm,
                        );
                        Frame::Cow(self_frame)
                    }
                }
            })
            .collect();

        forked_pages_frames
    }

    fn try_to_fix_memory_fault(
        &mut self,
        uaddr: usize,
        access_type: MemAccessType,
        pagetable: &RwLock<PageTable>,
    ) -> bool {
        if !self.perm().contains(MapPerm::W) && access_type == MemAccessType::Write {
            return false;
        }

        let page_index = (uaddr - self.ubase()) / crate::arch::PGSIZE;
        let perm = self.perm();
        if page_index >= self.page_count() {
            return false;
        }

        let old_frame = core::mem::replace(&mut self.page_frames()[page_index], Frame::Unallocated);

        let new_frame = match old_frame {
            Frame::Unallocated => {
                let allocated_page = self.alloc_new_page(page_index, pagetable);
                pagetable
                    .write()
                    .mmap(uaddr & !arch::PGMASK, allocated_page.get_page(), perm);
                Frame::Allocated(Arc::new(allocated_page))
            }
            Frame::Allocated(_) => {
                unreachable!();
            }
            Frame::Cow(cow_frame) => {
                // Copy-on-write: create a new copy for this process
                debug_assert!(
                    access_type == MemAccessType::Write,
                    "COW page accessed for read"
                );
                Frame::Allocated(Arc::new(match Arc::try_unwrap(cow_frame) {
                    Ok(only_cow_frame) => only_cow_frame,
                    Err(cow_frame) => self.alloc_for_cow_page(page_index, cow_frame),
                }))
            }
        };

        self.page_frames()[page_index] = new_frame;

        true
    }

    fn page_count(&self) -> usize;
    fn size(&self) -> usize {
        self.page_count() * crate::arch::PGSIZE
    }

    fn split(self: Box<Self>, _uaddr: usize) -> (Box<dyn Area>, Box<dyn Area>) {
        unimplemented!(
            "split not implemented for the area type: {}",
            self.type_name()
        );
    }

    fn set_perm(&mut self, _perm: MapPerm, _pagetable: &RwLock<PageTable>) {
        unimplemented!(
            "set_perm not implemented for the area type: {}",
            self.type_name()
        );
    }

    fn unmap(&mut self, _pagetable: &RwLock<PageTable>) {
        unimplemented!(
            "unmap not implemented for the area type: {}",
            self.type_name()
        );
    }

    fn type_name(&self) -> &'static str {
        "Area"
    }
}
