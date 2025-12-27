use core::time::Duration;

use crate::kernel::mm::MapPerm;

use super::{KernelContext, SigContext};

#[derive(Debug, Clone, Copy)]
pub struct MappedPage {
    pub kaddr: usize,
    pub perm: MapPerm,
}

pub trait PageTableTrait {
    fn mmap(&mut self, uaddr: usize, kaddr: usize, perm: MapPerm);
    fn mmap_paddr(&mut self, kaddr: usize, paddr: usize, perm: MapPerm);
    fn mmap_replace(&mut self, uaddr: usize, kaddr: usize, perm: MapPerm);
    fn mmap_replace_kaddr(&mut self, uaddr: usize, kaddr: usize);
    fn mmap_replace_perm(&mut self, uaddr: usize, perm: MapPerm);
    fn munmap(&mut self, uaddr: usize);
    fn munmap_with_check(&mut self, uaddr: usize, expected_kaddr: usize) -> bool;
    fn take_access_dirty_bit(&mut self, uaddr: usize) -> Option<(bool, bool)>;

    // fn mapped_page(&self, uaddr: usize) -> Option<MappedPage>;
    // fn munmap_if_mapped(&mut self, uaddr: usize) -> bool;
    // fn is_mapped(&self, uaddr: usize) -> bool;
}

pub trait ArchTrait {    
    fn init();
    fn setup_all_cores(current_core: usize);
    
    /* ----- Per-CPU Data ----- */
    fn set_percpu_data(data: usize);
    fn get_percpu_data() -> usize;

    /* ----- Context Switching ----- */
    fn kernel_switch(from: *mut KernelContext, to: *mut KernelContext);
    fn get_user_pc() -> usize;
    fn return_to_user() -> !;
    
    /* ----- Interrupt ------ */
    fn wait_for_interrupt();
    fn enable_interrupt();
    fn disable_interrupt();
    fn enable_timer_interrupt();
    fn enable_device_interrupt();
    fn enable_device_interrupt_irq(irq: u32);

    fn get_kernel_stack_top() -> usize;

    fn kaddr_to_paddr(kaddr: usize) -> usize;
    fn paddr_to_kaddr(paddr: usize) -> usize;
    fn scan_device();
    fn map_kernel_addr(kstart: usize, pstart: usize, size: usize, perm: MapPerm);
    unsafe fn unmap_kernel_addr(kstart: usize, size: usize);

    fn uptime() -> Duration;
    fn get_time_us() -> u64;
    fn set_next_time_event_us(interval: u64);

    fn read_volatile<T>(src: *const T) -> T;
    fn write_volatile<T>(dst: *mut T, val: T);
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

    fn set_sigaction_restorer(&mut self, uptr_restorer: usize) -> &mut Self;
    fn restore_from_signal(&mut self, sigcontext: &SigContext) -> &mut Self;
    fn set_arg(&mut self, index: usize, arg: usize) -> &mut Self;

    fn set_user_entry(&mut self, entry: usize) -> &mut Self;
    fn get_user_entry(&self) -> usize;
    fn skip_syscall_instruction(&mut self);
    fn set_tls(&mut self, tls: usize);
}

pub struct Arch;
