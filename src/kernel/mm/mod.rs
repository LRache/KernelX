pub mod elf;
pub mod page;
// mod frame;
mod addrspace;
pub mod maparea;
pub mod ubuf;
pub mod vdso;

pub use addrspace::*;
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
pub fn init(frame_start: usize, frame_end: usize) {
    page::init(frame_start, frame_end);
    // crate::kinfo!("Frame space inited: {:#x} - {:#x}, total {:#x}", frame_start, frame_end, frame_end - frame_start);
}
