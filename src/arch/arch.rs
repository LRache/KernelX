use crate::kernel::mm::MapPerm;

use super::{KernelContext, SigContext};

pub trait PageTableTrait {
    fn mmap(&mut self, uaddr: usize, kaddr: usize, perm: MapPerm);
    fn mmap_paddr(&mut self, kaddr: usize, paddr: usize, perm: MapPerm);
    fn mmap_replace(&mut self, uaddr: usize, kaddr: usize, perm: MapPerm);
    fn munmap(&mut self, uaddr: usize);
    // fn munmap_if_mapped(&mut self, uaddr: usize) -> bool;
    // fn is_mapped(&self, uaddr: usize) -> bool;
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

pub trait UserContextTrait: Clone {
    fn new() -> Self;
    
    /// Create a clone of the current context for fork. The returned context
    /// will return 0 in the user program.
    fn new_clone(&self) -> Self;

    fn get_user_stack_top(&self) -> usize;
    fn set_user_stack_top(&mut self, user_stack_top: usize);
    fn set_kernel_stack_top(&mut self, kernel_stack_top: usize);

    fn set_addrspace(&mut self, addrspace: &crate::kernel::mm::AddrSpace);

    fn set_sigaction_restorer(&mut self, uptr_restorer: usize);
    fn restore_from_signal(&mut self, sigcontext: &SigContext);

    fn set_user_entry(&mut self, entry: usize);
    fn get_user_entry(&self) -> usize;
    fn skip_syscall_instruction(&mut self);
}

pub struct Arch;
