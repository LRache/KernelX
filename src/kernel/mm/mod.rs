pub mod page;
pub mod elf;
mod frame;
mod addrspace;
pub mod vdso;
pub mod maparea;
// pub mod uptr;

pub use addrspace::*;
pub use frame::PhysPageFrame;

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

#[unsafe(link_section = ".text.init")]
pub fn init(frame_start: usize, frame_end: usize) {
    page::init(frame_start, frame_end);
    vdso::init();
}
