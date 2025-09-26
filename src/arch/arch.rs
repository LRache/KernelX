use crate::kernel::mm::MapPerm;
use crate::kernel::errno::SysResult;

use super::KernelContext;

pub trait PageTableTrait {
    fn mmap(&mut self, uaddr: usize, kaddr: usize, perm: MapPerm);
    fn mmap_paddr(&mut self, kaddr: usize, paddr: usize, perm: MapPerm);
    fn mmap_replace(&mut self, uaddr: usize, kaddr: usize, perm: MapPerm);
    fn munmap(&mut self, uaddr: usize);
    fn munmap_if_mapped(&mut self, uaddr: usize) -> bool;
    fn is_mapped(&self, uaddr: usize) -> bool;
    fn translate(&self, uaddr: usize) -> Option<usize>;
    fn mprotect(&mut self, uaddr: usize, perm: MapPerm) -> SysResult<()>;
}

pub trait ArchTrait {
    fn init();
    
    /* ----- Per-CPU Data ----- */
    fn set_percpu_data(data: usize);
    fn get_percpu_data() -> usize;

    /* ----- Context Switching ----- */
    fn kernel_switch(from: *mut KernelContext, to: *mut KernelContext);
    fn get_user_pc() -> usize;
    
    /* ----- Interrupt ------ */
    fn wait_for_interrupt();
    fn enable_interrupt();
    fn disable_interrupt();
    fn enable_timer_interrupt();
}

pub struct Arch;
