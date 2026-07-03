use core::time::Duration;

use crate::kernel::mm::MapPerm;

use super::{KernelContext, SigContext};

/// ABI hall of shame: some architectures (RISC-V) pass the `tls` argument to `clone` before the `ctid` argument,
/// while others (LoongArch) do the opposite.
/// The `CloneABI` enum and `ArchTrait::clone_abi()` method allow the kernel to abstract over this difference.
#[allow(dead_code)]
#[derive(Debug, Clone, Copy)]
pub enum CloneABI {
    /// flags, stack, ptid, ctid, tls
    Normal,

    /// flags, stack, ptid, tls, ctid
    Backwards,
}

pub trait PageTableTrait {
    fn mmap(&mut self, uaddr: usize, kaddr: usize, perm: MapPerm);
    fn mmap_replace(&mut self, uaddr: usize, kaddr: usize, perm: MapPerm);
    fn mmap_replace_perm(&mut self, uaddr: usize, perm: MapPerm);
    fn munmap(&mut self, uaddr: usize) -> Result<(), ()>;
    fn munmap_with_check(&mut self, uaddr: usize, expected_kaddr: usize) -> bool;

    #[allow(dead_code)]
    fn take_access_dirty_bit(&mut self, uaddr: usize) -> Option<(bool, bool)>;
}

pub trait ArchTrait {
    fn init(memory_top: usize);
    fn setup_all_cores(current_core: usize);
    fn clone_abi() -> CloneABI;

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
    fn enable_device_interrupt(hartid: usize);
    fn enable_device_interrupt_irq(irq: u32);

    #[allow(dead_code)]
    fn get_kernel_stack_top() -> usize;

    fn kaddr_to_paddr(kaddr: usize) -> usize;
    fn paddr_to_kaddr(paddr: usize) -> usize;
    fn scan_device();
    fn map_kernel_addr(kstart: usize, pstart: usize, size: usize, perm: MapPerm);
    unsafe fn unmap_kernel_addr(kstart: usize, size: usize);

    /// Translate a device MMIO physical address into a kernel-accessible VA
    /// suitable for volatile reads/writes. The returned address must resolve
    /// to the same physical region with *uncached* (device / strongly-ordered)
    /// semantics — cache coherence with DMA engines lives outside kernel
    /// control for these regions.
    ///
    /// - RISC-V: reserves kernel virtual addresses above `memory_top + kaddr_offset`
    ///   and installs a `MapPerm::RW` mapping via the kernel page table.
    /// - LoongArch: returns the DMW0 window mirror of the PA (no allocation,
    ///   no page-table edits — DMW0 is MAT=SUC, uncached by hardware).
    fn mmio_phys_to_kaddr(paddr: usize, size: usize) -> usize;

    fn uptime() -> Duration;
    fn get_time_us() -> u64;
    fn set_next_time_event_us(interval: u64);

    fn read_volatile<T>(src: *const T) -> T;
    fn write_volatile<T>(dst: *mut T, val: T);

    fn get_frame_pointer() -> usize;
    unsafe fn frame_info(fp: usize) -> (usize, usize);
    fn is_kernel_addr(addr: usize) -> bool;

    fn crc32c(seed: u32, buf: &[u8]) -> u32 {
        crate::klib::crc::crc32c_update(seed, buf)
    }
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
    fn move_back_to_syscall_instruction(&mut self);
    fn set_tls(&mut self, tls: usize);

    fn set_syscall_retval(&mut self, retval: usize);
}

pub struct Arch;
