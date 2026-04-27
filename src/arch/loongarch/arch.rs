//! LoongArch64 `ArchTrait` — Phase 4 cut.
//!
//! What's real now on top of Phase 3:
//!   - trap entry installed in EENTRY; kerneltrap_handler dispatches timer
//!     and panics readably on any other exception
//!   - stable-timer programmed for 10 ms ticks via TCFG
//!   - interrupt enable/disable/wait via CRMD.IE / `idle 0`
//!   - kernel_switch goes to the asm in clib/.../switch.S
//!
//! Still `unimplemented!()`: everything touching the user side
//! (get_user_pc / return_to_user / enable_device_interrupt / irq-specific
//! enable). Those are Phase 5/6.

use core::time::Duration;

use crate::arch::arch::{Arch, ArchTrait, UserContextTrait};
use crate::driver::chosen;
use crate::kernel::mm::MapPerm;
use crate::klib::initcell::InitedCell;

use super::boot::EARLY_UART;
use super::context::KernelContext;
use super::csr;
use super::eiointc;
use super::fdt;
use super::pch_pic;
use super::task;
use super::trap;

/// DMW1 base. Set in `clib/src/arch/loongarch/entry/entry.S`. Keeps
/// VA = PA | DMW1_MASK for every kernel byte we ever touch.
const DMW1_MASK: usize = 0x9000_0000_0000_0000;
/// Low 48 bits — PALEN=48 on la464, any kaddr ANDed with this becomes PA.
const PA_MASK: usize = (1 << 48) - 1;

/// Stable-counter frequency, cached at `Arch::init` so `get_time_us` doesn't
/// hit CPUCFG on every call. 100 MHz on QEMU virt la464.
static STABLE_COUNTER_FREQ_HZ: InitedCell<u64> = InitedCell::uninit();

impl ArchTrait for Arch {
    fn init() {
        // Kernel console first, so anything that follows (including a panic)
        // can actually talk to the outside world.
        chosen::kconsole::register(&EARLY_UART);

        // Trap entry: install EENTRY + set VS=0 (single-entry, Ecode-dispatched).
        trap::install_trap_entry();

        // Configure the hardware page-table walker so that, as soon as a
        // user task is scheduled (Phase 5), CPU-side TLB misses auto-walk
        // our 3-level page table. This is global state (not per-process),
        // so we do it once here.
        //
        // STLBPS = 12 (log2 of 4 KiB page size)
        // PWCL   = 3-level 9-9-9-12 layout with 64-bit PTEs
        // PWCH   = 0 (no 4th level)
        // ASID   = 0 — Phase 5 uses a single ASID and relies on full TLB
        //           flushes via invtlb when page tables change.
        csr::write::<{ csr::num::STLBPS }>(csr::stlbps::PS_4K);
        csr::write::<{ csr::num::PWCL   }>(csr::pwcl::THREE_LEVEL_9_9_9_12);
        csr::write::<{ csr::num::PWCH   }>(csr::pwch::NONE);
        csr::write::<{ csr::num::ASID   }>(0);

        // Cache the stable-counter frequency for get_time_us. Must be done
        // before any code path calls uptime() / timer::now().
        STABLE_COUNTER_FREQ_HZ.init(csr::stable_counter_freq());

        // We're ready to take interrupts as soon as CRMD.IE gets flipped by
        // the scheduler loop. No interrupt lines are enabled yet; Phase 4's
        // enable_timer_interrupt fires first (from main() after this init).
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

    fn kernel_switch(from: *mut KernelContext, to: *mut KernelContext) {
        task::kernel_switch(from, to);
    }

    fn get_user_pc() -> usize {
        crate::kernel::scheduler::current::tcb().user_context().get_user_entry()
    }

    fn return_to_user() -> ! {
        task::traphandle::return_to_user()
    }

    /* ----- Interrupts (CRMD.IE / ECFG.LIE) ----- */

    fn wait_for_interrupt() {
        unsafe { core::arch::asm!("idle 0", options(nostack, preserves_flags)) };
    }

    fn enable_interrupt() {
        csr::xchg::<{ csr::num::CRMD }>(csr::crmd::IE, csr::crmd::IE);
    }

    fn disable_interrupt() {
        csr::xchg::<{ csr::num::CRMD }>(0, csr::crmd::IE);
    }

    fn enable_timer_interrupt() {
        // ECFG.LIE bit 11 — "timer interrupt line enabled at the core".
        let bit = 1usize << csr::ecfg::LINE_TIMER;
        csr::xchg::<{ csr::num::ECFG }>(bit, bit);
    }

    fn enable_device_interrupt(_hartid: usize) {
        // Phase 6: HWI0 is the one line we care about — EIOINTC fans out to
        // it, so switching HWI0 on at the ECFG.LIE level is enough. Actual
        // EIOINTC config (MISC bits, ROUTE/IPMAP tables) happens once in
        // `eiointc::init()` during `scan_device`.
        //
        // This runs on every hart via main(), so on SMP it'd need to run
        // everywhere. Phase 8 territory; single-core for now.
        let bit = 1usize << csr::ecfg::LINE_HWI0;
        csr::xchg::<{ csr::num::ECFG }>(bit, bit);
    }

    fn enable_device_interrupt_irq(irq: u32) {
        // Pass-through: PCH-PIC IRQ N → EIOINTC IRQ N. Both layers must
        // be unmasked for the IRQ to reach the CPU.
        pch_pic::enable_irq(irq);
        eiointc::enable_irq(irq);
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
        //   - MMIO callers must not reach this — they go through
        //     `mmio_phys_to_kaddr` instead, which returns the DMW0 mirror
        //     (uncached) so volatile accesses bypass the cache.
    }

    unsafe fn unmap_kernel_addr(_kstart: usize, _size: usize) {
        // See `map_kernel_addr` above. This is intentionally a no-op on
        // LoongArch; there is no kernel page table to edit.
    }

    fn mmio_phys_to_kaddr(paddr: usize, _size: usize) -> usize {
        // DMW0 @ 0x8000_0000_0000_0000: VSEG=0x8, MAT=SUC (uncached,
        // strongly-ordered). Every device MMIO register is reachable by
        // ORing the PA with this base — no allocation, no TLB flush, no
        // page-table edit. entry.S programmed DMW0 during early boot so
        // this mirror is live for the entire kernel lifetime.
        //
        // Callers of this function want MMIO semantics; `paddr_to_kaddr`
        // returns the DMW1 (cached) mirror and MUST NOT be used for
        // device registers — cache coherency with a DMA-capable device
        // is not something the OS can enforce on LA without an explicit
        // barrier + cacheop dance per access.
        const DMW0_MASK: usize = 0x8000_0000_0000_0000;
        debug_assert!(paddr < (1usize << 48), "PA {:#x} outside PALEN=48", paddr);
        paddr | DMW0_MASK
    }

    /* ----- Time ----- */

    fn uptime() -> Duration {
        Duration::from_micros(Self::get_time_us())
    }

    fn get_time_us() -> u64 {
        // rdtime.d gives us raw counter ticks. Convert with the cached freq.
        // Integer math order matters: multiply by 1_000_000 first so small
        // intervals don't round to zero. The counter is 64-bit — we have
        // ~5800 years of headroom before overflow at 100 MHz, so the
        // multiply is safe.
        csr::rdtime() * 1_000_000 / *STABLE_COUNTER_FREQ_HZ
    }

    fn set_next_time_event_us(interval: u64) {
        // Convert microseconds → counter ticks. The scheduler passes 10_000
        // (= 10 ms); at 100 MHz that's 1_000_000 ticks, well clear of the
        // 2-bit shift in TCFG.
        let ticks = (interval * *STABLE_COUNTER_FREQ_HZ) / 1_000_000;
        let tcfg = (ticks as usize) << csr::tcfg::INITVAL_SHIFT
            | csr::tcfg::PERIODIC
            | csr::tcfg::EN;
        csr::write::<{ csr::num::TCFG }>(tcfg);
    }

    fn scan_device() {
        // Walks the FDT QEMU's loongarch_direct_kernel_boot path put at
        // the fixed PA 0x100000 (see fdt.rs::FDT_BASE_PA). Initializes
        // EIOINTC + PCH-PIC, then registers every other top-level node
        // via the driver matcher. /chosen/bootargs drives `parse_boot_args`.
        if let Err(()) = fdt::load_device_tree() {
            crate::kwarn!("loongarch: FDT walk failed; continuing without devices");
        }
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
