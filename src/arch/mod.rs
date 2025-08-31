#[cfg(target_arch = "riscv64")]
mod riscv;
#[cfg(target_arch = "riscv64")]
pub use riscv::*;

use crate::kernel::mm::MapPerm;
use crate::kernel::errno::Errno;

pub trait PageTableTrait {
    fn mmap(&mut self, uaddr: usize, kaddr: usize, perm: MapPerm);
    fn mmap_paddr(&mut self, kaddr: usize, paddr: usize, perm: MapPerm);
    fn mmap_replace(&mut self, uaddr: usize, kaddr: usize, perm: MapPerm);
    fn munmap(&mut self, uaddr: usize);
    fn munmap_if_mapped(&mut self, uaddr: usize) -> bool;
    fn is_mapped(&self, uaddr: usize) -> bool;
    fn translate(&self, uaddr: usize) -> Option<usize>;
    fn mprotect(&mut self, uaddr: usize, perm: MapPerm) -> Result<(), Errno>;
}
