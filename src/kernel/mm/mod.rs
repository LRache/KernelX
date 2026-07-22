pub mod elf;
pub mod page;
// mod frame;
mod addrspace;
pub mod maparea;
mod memregion;
pub mod ubuf;
pub mod vdso;

pub use addrspace::*;
pub use memregion::{MemRegion, contains_range as contains_memory_range, max_end as max_memory_end};
pub use page::*;

pub mod swappable;

use bitflags::bitflags;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MemAccessType {
    Read,
    Write,
    Execute,
}

bitflags! {
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub struct MapPerm: u8 {
        const R = 1 << 0;
        const W = 1 << 1;
        const X = 1 << 2;
        const U = 1 << 3;
        const RW = (1 << 0) | (1 << 1);
    }
}

impl MemAccessType {
    pub fn match_perm(&self, perm: MapPerm) -> bool {
        match self {
            MemAccessType::Read => perm.contains(MapPerm::R),
            MemAccessType::Write => perm.contains(MapPerm::W),
            MemAccessType::Execute => perm.contains(MapPerm::X),
        }
    }
}

#[unsafe(link_section = ".text.init")]
pub unsafe fn init(bootstrap_end: usize, regions: *const MemRegion, region_count: usize) {
    // SAFETY: The architecture entry code reserves the region array page and
    // passes the number of initialized entries in that page.
    let regions = unsafe { memregion::init(regions, region_count) };
    page::init(bootstrap_end, regions);
}
