pub mod page;
pub mod elf;
mod frame;
mod addrspace;
pub mod vdso;
pub mod maparea;

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
    }
}

pub fn init(heap_end: usize) {
    page::init(heap_end);
    vdso::init();
}
