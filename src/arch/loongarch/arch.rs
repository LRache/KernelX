//! LoongArch64 `ArchTrait` — Phase 2.5 cut.
//!
//! What's real:
//!   - per-CPU data pointer stashed in `$r21` (the kernel-reserved GPR)
//!   - DMW1 / DMW0 address translation (pure bit twiddling, no page table)
//!   - kernel console registration via `driver::chosen::kconsole`
//!   - frame pointer / kernel stack top (for backtrace + overflow checks)
//!   - is_kernel_addr (carry-over from Phase 1)
//!
//! Everything else still `unimplemented!()`s. Ordering mirrors `src/arch/riscv/arch.rs`.

use core::time::Duration;

use crate::arch::arch::{Arch, ArchTrait};
use crate::driver::chosen;
use crate::kernel::mm::MapPerm;

use super::boot::EARLY_UART;
use super::context::KernelContext;

/// DMW1 base. Set in `clib/src/arch/loongarch/entry/entry.S`. Keeps
/// VA = PA | DMW1_MASK for every kernel byte we ever touch.
const DMW1_MASK: usize = 0x9000_0000_0000_0000;
/// Low 48 bits — PALEN=48 on la464, any kaddr ANDed with this becomes PA.
const PA_MASK: usize = (1 << 48) - 1;

impl ArchTrait for Arch {
    fn init() {
        // Kernel console first, so anything that follows (including a panic)
        // can actually talk to the outside world.
        chosen::kconsole::register(&EARLY_UART);
        // Phase 4 will set EENTRY / TLBRENTRY, init the timer, etc. For now
        // we leave them zero — any exception at this stage means a real bug
        // and we'd rather triple-fault than mask it.
    }

    fn setup_all_cores(_current_core: usize) {
        // LoongArch SMP bring-up uses IPI CSRs (not SBI) and lives in Phase 8.
        // On NO_SMP builds there's nothing to do; mirror the RISC-V convention
        // of a loop whose body is empty at core_count() == 1 rather than
        // guarding with cfg, so the shape stays consistent across arches.
    }

    /* ----- Per-CPU Data (stashed in $r21, the kernel-reserved reg) ----- */

    #[inline(always)]
    fn set_percpu_data(data: usize) {
        unsafe { core::arch::asm!("move $r21, {x}", x = in(reg) data) };
    }

    #[inline(always)]
    fn get_percpu_data() -> usize {
        let data: usize;
        unsafe { core::arch::asm!("move {x}, $r21", x = out(reg) data) };
        data
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

    #[inline(always)]
    fn get_kernel_stack_top() -> usize {
        let sp: usize;
        unsafe { core::arch::asm!("move {x}, $sp", x = out(reg) sp) };
        sp
    }

    /* ----- Address translation via the DMW1 window -----
     * DMW1 is programmed at boot (entry.S) with VSEG=0x9, MAT=CC, PLV0, so
     * every kernel byte lives at VA = PA | DMW1_MASK. These two helpers are
     * called all over kernel/mm (the hot path for page alloc), so keep them
     * branchless.
     */

    #[inline(always)]
    fn kaddr_to_paddr(kaddr: usize) -> usize {
        kaddr & PA_MASK
    }

    #[inline(always)]
    fn paddr_to_kaddr(paddr: usize) -> usize {
        paddr | DMW1_MASK
    }

    fn map_kernel_addr(_kstart: usize, _pstart: usize, _size: usize, _perm: MapPerm) {
        // DMW0 (VSEG 0x8, MAT=SUC) covers MMIO and DMW1 (VSEG 0x9, MAT=CC)
        // covers RAM — both are programmed in clib/.../entry.S and have
        // priority over the TLB. Any kernel VA of ours (`paddr | 0x9000_...`)
        // resolves directly via DMW; there is no kernel page table to edit,
        // which is why the RISC-V kernelpagetable machinery has no LA analog.
        //
        // Consequences:
        //   - `KernelStack::new` on LA silently loses its hardware guard page.
        //     The software overflow check in `KernelStack::check_stack_overflow`
        //     is our only protection.
        //   - Driver MMIO callers compute `kbase = paddr_to_kaddr(pa)` which
        //     lands in the DMW1 cached window. That's semantically wrong for
        //     uncached MMIO, but we don't trip it in Phase 3 (scan_device is
        //     a no-op). Phase 6 will have to route MMIO through DMW0 — see
        //     `paddr_to_kaddr` in this file for the fix site.
    }

    unsafe fn unmap_kernel_addr(_kstart: usize, _size: usize) {
        // See `map_kernel_addr` above. This is intentionally a no-op on
        // LoongArch; there is no kernel page table to edit.
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
        // Phase 6 will walk the FDT that QEMU pins at PA 0x100000 and populate
        // `driver::found_device` + `BOOT_ARGS`. For now a log line so the
        // absence is visible.
        crate::kinfo!("loongarch: scan_device is a no-op (Phase 6 will parse the FDT)");
    }

    /* ----- Volatile fences ----- */

    fn read_volatile<T>(src: *const T) -> T {
        unsafe {
            let v = core::ptr::read_volatile(src);
            core::arch::asm!("dbar 0", options(nostack, preserves_flags));
            v
        }
    }

    fn write_volatile<T>(dst: *mut T, val: T) {
        unsafe {
            core::arch::asm!("dbar 0", options(nostack, preserves_flags));
            core::ptr::write_volatile(dst, val);
        }
    }

    /* ----- Debugging helpers (fp = $r22) ----- */

    #[inline(always)]
    fn get_frame_pointer() -> usize {
        let fp: usize;
        unsafe { core::arch::asm!("move {x}, $r22", x = out(reg) fp) };
        fp
    }

    #[inline(always)]
    unsafe fn frame_info(fp: usize) -> (usize, usize) {
        // LoongArch gcc/clang with -fno-omit-frame-pointer places (ra, old_fp)
        // at the top of each frame, right below the saved fp. This matches
        // the RISC-V port's convention, so `klib::backtrace` stays arch-agnostic.
        let p = fp as *const usize;
        unsafe { (*p.sub(1), *p.sub(2)) }
    }

    #[inline(always)]
    fn is_kernel_addr(addr: usize) -> bool {
        // DMW0/1 live in the upper half (bit 63 set). Every kernel VA comes
        // out of paddr_to_kaddr, which OR-s in DMW1_MASK.
        (addr as isize) < 0
    }
}
