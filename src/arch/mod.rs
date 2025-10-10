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

// pub const PGBITS: usize = arch_impl::PGBITS;
pub const PGSIZE: usize = arch_impl::PGSIZE;
pub const PGMASK: usize = arch_impl::PGMASK;

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

arch_export! {
    init() -> ();
    
    /* ----- Per-CPU Data ----- */
    set_percpu_data(data: usize) -> ();
    get_percpu_data() -> usize;

    /* ----- Context Switching ----- */
    kernel_switch(from: *mut KernelContext, to: *mut KernelContext) -> ();
    get_user_pc() -> usize;
    
    /* ----- Interrupt ------ */
    wait_for_interrupt() -> ();
    enable_interrupt  () -> ();
    disable_interrupt () -> ();
    enable_timer_interrupt() -> ();
}
