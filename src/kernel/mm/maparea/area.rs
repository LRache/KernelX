use alloc::boxed::Box;
use alloc::sync::Arc;
use spin::RwLock;

use crate::kernel::mm::{AddrSpace, MapPerm, MemAccessType};
use crate::kernel::mm::PhysPageFrame;
use crate::arch::PageTable;

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
    fn translate_read (&mut self, uaddr: usize, addrspace: &AddrSpace) -> Option<usize>;
    fn translate_write(&mut self, uaddr: usize, addrspace: &AddrSpace) -> Option<usize>;
    
    fn ubase(&self) -> usize;
    
    fn set_ubase(&mut self, _ubase: usize) {
        unimplemented!("set_ubase not implemented for the area type: {}", self.type_name());
    }
    
    fn perm(&self) -> MapPerm;

    fn fork(&mut self, self_pagetable: &RwLock<PageTable>, fork_pagetable: &RwLock<PageTable>) -> Box<dyn Area>;

    fn try_to_fix_memory_fault(
        &mut self, 
        uaddr: usize, 
        access_type: MemAccessType, 
        addrspace: &Arc<AddrSpace>
    ) -> bool;

    fn page_count(&self) -> usize;
    fn size(&self) -> usize {
        self.page_count() * crate::arch::PGSIZE
    }

    fn split(self: Box<Self>, _uaddr: usize) -> (Box<dyn Area>, Box<dyn Area>) {
        unimplemented!("split not implemented for the area type: {}", self.type_name());
    }
    
    fn set_perm(&mut self, _perm: MapPerm, _pagetable: &RwLock<PageTable>) {
        unimplemented!("set_perm not implemented for the area type: {}", self.type_name());
    }
    
    fn unmap(&mut self, _pagetable: &RwLock<PageTable>) {
        unimplemented!("unmap not implemented for the area type: {}", self.type_name());
    }

    fn type_name(&self) -> &'static str {
        "Area"
    }
}
