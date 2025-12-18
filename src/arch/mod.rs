cfg_if::cfg_if! {
    if #[cfg(target_arch = "riscv64")] {
        mod riscv;
        use riscv as arch_impl;
    } else {
        compile_error!("Unsupported architecture");
    }
}

pub type UserContext = arch_impl::UserContext;
pub type KernelContext = arch_impl::KernelContext;
pub type SigContext = arch_impl::SigContext;
pub type PageTable = arch_impl::PageTable;
// pub type MappedPage<'a> = arch_impl::MappedPage<'a>;

pub const PGSIZE: usize = arch_impl::PGSIZE;
pub const PGMASK: usize = arch_impl::PGMASK;
pub const TRAMPOLINE_BASE: usize = arch_impl::TRAMPOLINE_BASE;

mod arch;
pub use arch::{PageTableTrait, UserContextTrait};
use arch::{Arch, ArchTrait};

macro_rules! arch_export {
    ($($func:ident($($arg:ident: $type:ty),*) -> $ret:ty);* $(;)?) => {
        $(
            pub fn $func($($arg: $type),*) -> $ret {
                Arch::$func($($arg),*)
            }
        )*
    };
}

use core::time::Duration;
use crate::kernel::mm::MapPerm;

arch_export! {
    init() -> ();
    setup_all_cores(current_core: usize) -> ();
    
    /* ----- Per-CPU Data ----- */
    set_percpu_data(data: usize) -> ();
    get_percpu_data() -> usize;

    /* ----- Context Switching ----- */
    kernel_switch(from: *mut KernelContext, to: *mut KernelContext) -> ();
    get_user_pc() -> usize;
    return_to_user() -> !;
    
    /* ----- Interrupt ------ */
    wait_for_interrupt() -> ();
    enable_interrupt  () -> ();
    disable_interrupt () -> ();
    enable_timer_interrupt() -> ();
    enable_device_interrupt() -> ();
    enable_device_interrupt_irq(irq: u32) -> ();

    get_kernel_stack_top() -> usize;

    // kaddr_offset() -> usize;
    kaddr_to_paddr(kaddr: usize) -> usize;
    paddr_to_kaddr(paddr: usize) -> usize;
    map_kernel_addr(kstart: usize, pstart: usize, size: usize, perm: MapPerm) -> ();

    get_time_us() -> u64;
    uptime() -> Duration;
    set_next_time_event_us(internval: u64) -> ();

    scan_device() -> ();
}

pub fn page_count(size: usize) -> usize {
    (size + PGSIZE - 1) / PGSIZE
}

pub unsafe fn unmap_kernel_addr(kstart: usize, size: usize) {
    unsafe { Arch::unmap_kernel_addr(kstart, size) }
}
