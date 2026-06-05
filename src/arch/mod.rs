cfg_if::cfg_if! {
    if #[cfg(target_arch = "riscv64")] {
        mod riscv;
        use riscv as arch_impl;
    } else if #[cfg(target_arch = "loongarch64")] {
        mod loongarch;
        use loongarch as arch_impl;
    } else {
        compile_error!("Unsupported architecture");
    }
}

pub type UserContext = arch_impl::UserContext;
pub type KernelContext = arch_impl::KernelContext;
pub type SigContext = arch_impl::SigContext;
pub type PageTable = arch_impl::PageTable;
// pub type MappedPage<'a> = arch_impl::MappedPage<'a>;

cfg_if::cfg_if! {
    if #[cfg(feature = "kvm")] {
        pub use arch_impl::{KvmPageFault, KvmPageTable, KvmRegs, KvmSRegs, VCpu};
    }
}

pub const PGSIZE: usize = arch_impl::PGSIZE;
pub const PGMASK: usize = arch_impl::PGMASK;
pub const USEREND: usize = arch_impl::USEREND;

mod arch;
use arch::{Arch, ArchTrait};
pub use arch::{CloneABI, PageTableTrait, UserContextTrait};

macro_rules! arch_export {
    ($($func:ident($($arg:ident: $type:ty),*) -> $ret:ty);* $(;)?) => {
        $(
            pub fn $func($($arg: $type),*) -> $ret {
                Arch::$func($($arg),*)
            }
        )*
    };
}

use crate::kernel::mm::MapPerm;
use core::time::Duration;

arch_export! {
    init(memory_top: usize) -> ();
    setup_all_cores(current_core: usize) -> ();
    clone_abi() -> CloneABI;

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
    enable_device_interrupt(hartid: usize) -> ();
    enable_device_interrupt_irq(irq: u32) -> ();

    get_kernel_stack_top() -> usize;

    kaddr_to_paddr(kaddr: usize) -> usize;
    paddr_to_kaddr(paddr: usize) -> usize;
    map_kernel_addr(kstart: usize, pstart: usize, size: usize, perm: MapPerm) -> ();
    mmio_phys_to_kaddr(paddr: usize, size: usize) -> usize;

    get_time_us() -> u64;
    uptime() -> Duration;
    set_next_time_event_us(internval: u64) -> ();

    scan_device() -> ();

    is_kernel_addr(addr: usize) -> bool;
}

#[allow(dead_code)]
#[inline(always)]
pub fn get_frame_pointer() -> usize {
    Arch::get_frame_pointer()
}

#[allow(dead_code)]
#[inline(always)]
pub unsafe fn frame_info(fp: usize) -> (usize, usize) {
    unsafe { Arch::frame_info(fp) }
}

pub unsafe fn read_volatile<T>(src: *const T) -> T {
    Arch::read_volatile(src)
}

pub unsafe fn write_volatile<T>(dst: *mut T, val: T) {
    Arch::write_volatile(dst, val)
}

pub fn page_count(size: usize) -> usize {
    size.div_ceil(PGSIZE)
}

pub unsafe fn unmap_kernel_addr(kstart: usize, size: usize) {
    unsafe { Arch::unmap_kernel_addr(kstart, size) }
}
