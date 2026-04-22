//! LoongArch64 `ArchTrait` skeleton.
//!
//! Every runtime method panics — that's by design: Phase 1 only requires the
//! generic kernel code to *link*. Phase 2 onwards fills these in. The order
//! here mirrors `src/arch/riscv/arch.rs` so side-by-side diffs stay small.

use core::time::Duration;

use crate::arch::arch::{Arch, ArchTrait};
use crate::kernel::mm::MapPerm;

use super::context::KernelContext;

impl ArchTrait for Arch {
    fn init() {
        unimplemented!("loongarch: Arch::init (Phase 2/4)");
    }

    fn setup_all_cores(_current_core: usize) {
        // SMP boot on LoongArch uses IPI CSRs (not SBI). Implemented in Phase 8.
        unimplemented!("loongarch: Arch::setup_all_cores (Phase 8)");
    }

    /* ----- Per-CPU Data (stashed in $r21, the kernel-reserved reg) ----- */

    fn set_percpu_data(_data: usize) {
        unimplemented!("loongarch: Arch::set_percpu_data (Phase 2)");
    }

    fn get_percpu_data() -> usize {
        unimplemented!("loongarch: Arch::get_percpu_data (Phase 2)");
    }

    /* ----- Context Switching ----- */

    fn kernel_switch(_from: *mut KernelContext, _to: *mut KernelContext) {
        unimplemented!("loongarch: Arch::kernel_switch (Phase 4/5)");
    }

    fn get_user_pc() -> usize {
        unimplemented!("loongarch: Arch::get_user_pc (Phase 5)");
    }

    fn return_to_user() -> ! {
        unimplemented!("loongarch: Arch::return_to_user (Phase 5)");
    }

    /* ----- Interrupts (CRMD.IE / ECFG.LIE) ----- */

    fn wait_for_interrupt() {
        // Real impl: `idle 0`
        unimplemented!("loongarch: Arch::wait_for_interrupt (Phase 4)");
    }

    fn enable_interrupt() {
        unimplemented!("loongarch: Arch::enable_interrupt (Phase 4)");
    }

    fn disable_interrupt() {
        unimplemented!("loongarch: Arch::disable_interrupt (Phase 4)");
    }

    fn enable_timer_interrupt() {
        unimplemented!("loongarch: Arch::enable_timer_interrupt (Phase 4)");
    }

    fn enable_device_interrupt(_hartid: usize) {
        unimplemented!("loongarch: Arch::enable_device_interrupt (Phase 4/6)");
    }

    fn enable_device_interrupt_irq(_irq: u32) {
        unimplemented!("loongarch: Arch::enable_device_interrupt_irq (Phase 4/6)");
    }

    fn get_kernel_stack_top() -> usize {
        unimplemented!("loongarch: Arch::get_kernel_stack_top (Phase 2)");
    }

    /* ----- Address translation via DMW1 window (Phase 3 firms this up) ----- */

    fn kaddr_to_paddr(_kaddr: usize) -> usize {
        unimplemented!("loongarch: Arch::kaddr_to_paddr (Phase 2/3)");
    }

    fn paddr_to_kaddr(_paddr: usize) -> usize {
        unimplemented!("loongarch: Arch::paddr_to_kaddr (Phase 2/3)");
    }

    fn map_kernel_addr(_kstart: usize, _pstart: usize, _size: usize, _perm: MapPerm) {
        unimplemented!("loongarch: Arch::map_kernel_addr (Phase 3)");
    }

    unsafe fn unmap_kernel_addr(_kstart: usize, _size: usize) {
        unimplemented!("loongarch: Arch::unmap_kernel_addr (Phase 3)");
    }

    /* ----- Time ----- */

    fn uptime() -> Duration {
        Duration::from_micros(Self::get_time_us())
    }

    fn get_time_us() -> u64 {
        // Phase 4: `rdtime.d rd, rj` + CPUCFG-reported timer frequency
        unimplemented!("loongarch: Arch::get_time_us (Phase 4)");
    }

    fn set_next_time_event_us(_interval: u64) {
        // Phase 4: write TCFG with {InitVal<<2 | Periodic | Enable}
        unimplemented!("loongarch: Arch::set_next_time_event_us (Phase 4)");
    }

    fn scan_device() {
        unimplemented!("loongarch: Arch::scan_device (Phase 6)");
    }

    /* ----- Volatile fences ----- */

    fn read_volatile<T>(_src: *const T) -> T {
        unimplemented!("loongarch: Arch::read_volatile (Phase 4)");
    }

    fn write_volatile<T>(_dst: *mut T, _val: T) {
        unimplemented!("loongarch: Arch::write_volatile (Phase 4)");
    }

    /* ----- Debugging helpers (fp = $r22) ----- */

    fn get_frame_pointer() -> usize {
        unimplemented!("loongarch: Arch::get_frame_pointer (Phase 2/9)");
    }

    unsafe fn frame_info(_fp: usize) -> (usize, usize) {
        unimplemented!("loongarch: Arch::frame_info (Phase 9)");
    }

    fn is_kernel_addr(addr: usize) -> bool {
        // DMW0/1 live in the upper half (bit 63 set). Phase 3 may refine this
        // to a window-aware check, but the top-bit test is already correct for
        // all addresses we ever hand back up from the kernel.
        (addr as isize) < 0
    }
}
