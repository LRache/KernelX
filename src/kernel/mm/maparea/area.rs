use alloc::boxed::Box;
use alloc::string::String;
use alloc::sync::Arc;

use crate::arch::PageTable;
use crate::kernel::mm::{AddrSpace, MapPerm, MemAccessType, PhysPageFrame};
use crate::klib::SpinLock;

#[derive(Clone, Debug)]
pub struct MapAreaInfo {
    pub start: usize,
    pub end: usize,
    pub perm: MapPerm,
    pub shared: bool,
    pub offset: usize,
    pub dev_major: u32,
    pub dev_minor: u32,
    pub inode: u64,
    pub path: Option<String>,
}

impl MapAreaInfo {
    pub fn new(start: usize, end: usize, perm: MapPerm) -> Self {
        Self {
            start,
            end,
            perm,
            shared: false,
            offset: 0,
            dev_major: 0,
            dev_minor: 0,
            inode: 0,
            path: None,
        }
    }
}

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
    fn translate_read(&mut self, uaddr: usize, addrspace: &AddrSpace) -> Option<usize>;
    fn translate_write(&mut self, uaddr: usize, addrspace: &AddrSpace) -> Option<usize>;

    fn ubase(&self) -> usize;

    fn set_ubase(&mut self, _ubase: usize) {
        unimplemented!("set_ubase not implemented for the area type: {}", self.type_name());
    }

    fn perm(&self) -> MapPerm;

    fn fork(&mut self, self_pagetable: &SpinLock<PageTable>, fork_pagetable: &mut PageTable) -> Box<dyn Area>;

    fn try_to_fix_memory_fault(
        &mut self,
        uaddr: usize,
        access_type: MemAccessType,
        addrspace: &AddrSpace,
    ) -> Result<usize, MemoryFaultSignal>;

    fn page_count(&self) -> usize;
    fn size(&self) -> usize {
        self.page_count() * crate::arch::PGSIZE
    }

    fn split(self: Box<Self>, _uaddr: usize) -> (Box<dyn Area>, Box<dyn Area>) {
        unimplemented!("split not implemented for the area type: {}", self.type_name());
    }

    fn set_perm(&mut self, _perm: MapPerm, _pagetable: &SpinLock<PageTable>) {
        unimplemented!("set_perm not implemented for the area type: {}", self.type_name());
    }

    fn unmap(&mut self, _pagetable: &SpinLock<PageTable>) {
        unimplemented!("unmap not implemented for the area type: {}", self.type_name());
    }

    fn type_name(&self) -> &'static str {
        "Area"
    }

    fn map_area_info(&self) -> MapAreaInfo {
        MapAreaInfo::new(self.ubase(), self.ubase() + self.size(), self.perm())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MemoryFaultSignal {
    Segv,
    Bus,
}
