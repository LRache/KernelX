//! LoongArch64 `ArchTrait` implementation.

use alloc::boxed::Box;
use core::time::Duration;
use elf::abi;

use crate::arch::arch::{Arch, ArchTrait, CloneABI, UserContextTrait};
use crate::driver::chosen;
use crate::kernel::errno::SysResult;
use crate::kernel::mm::{self, MapPerm};
use crate::klib::initcell::InitedCell;
use crate::kmodule::{KModuleRelocationAction, KModuleRelocationValue};

use super::boot::EarlyUart;
use super::context::KernelContext;
use super::{csr, eiointc, fdt, pch_pic, task, trap};

const DMW1_MASK: usize = 0x9000_0000_0000_0000;
const PA_MASK: usize = (1 << 48) - 1;

unsafe extern "C" {
    static __kernel_start: u8;
}

static DIRECT_MAP_END: InitedCell<usize> = InitedCell::uninit();
static STABLE_COUNTER_FREQ_HZ: InitedCell<u64> = InitedCell::uninit();

impl Arch {
    pub fn try_uptime() -> Option<Duration> {
        let freq = *STABLE_COUNTER_FREQ_HZ.try_get()?;
        if freq == 0 {
            return None;
        }
        Some(Duration::from_micros(csr::rdtime().saturating_mul(1_000_000) / freq))
    }
}

impl ArchTrait for Arch {
    fn init() {
        DIRECT_MAP_END.init(Self::paddr_to_kaddr(mm::max_memory_end()));
        chosen::kconsole::register(Box::new(EarlyUart));

        STABLE_COUNTER_FREQ_HZ.init(csr::stable_counter_freq());
    }

    fn init_percpu() {
        trap::install_trap_entry();

        csr::write::<{ csr::num::STLBPS }>(csr::stlbps::PS_4K);
        csr::write::<{ csr::num::PWCL }>(csr::pwcl::THREE_LEVEL_9_9_9_12);
        csr::write::<{ csr::num::PWCH }>(csr::pwch::NONE);
        csr::write::<{ csr::num::ASID }>(0);

        const TLBREHI_PS_SHIFT: usize = 24;
        csr::write::<{ csr::num::TLBREHI }>(12usize << TLBREHI_PS_SHIFT);
    }

    fn setup_all_cores(_current_core: usize) {}

    fn clone_abi() -> CloneABI {
        CloneABI::Normal
    }

    fn cpu_count() -> usize {
        1
    }

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

    fn kernel_switch(from: *mut KernelContext, to: *mut KernelContext) {
        task::kernel_switch(from, to);
    }

    fn get_user_pc() -> usize {
        crate::kernel::scheduler::current::tcb().user_context().get_user_entry()
    }

    fn return_to_user() -> ! {
        task::traphandle::return_to_user()
    }

    fn wait_for_interrupt() {
        Self::enable_interrupt();
        // SAFETY: `idle 0` only suspends this core until an interrupt arrives.
        unsafe { core::arch::asm!("idle 0", options(nostack, preserves_flags)) };
    }

    fn enable_interrupt() {
        csr::xchg::<{ csr::num::CRMD }>(csr::crmd::IE, csr::crmd::IE);
    }

    fn disable_interrupt() {
        csr::xchg::<{ csr::num::CRMD }>(0, csr::crmd::IE);
    }

    fn enable_software_interrupt() {}

    fn enable_timer_interrupt() {
        let bit = 1usize << csr::ecfg::LINE_TIMER;
        csr::xchg::<{ csr::num::ECFG }>(bit, bit);
    }

    fn enable_device_interrupt(_hartid: usize) {
        let bit = 1usize << eiointc::parent_line();
        csr::xchg::<{ csr::num::ECFG }>(bit, bit);
    }

    fn enable_device_interrupt_irq(irq: u32) {
        if pch_pic::contains_irq(irq) {
            pch_pic::enable_irq(irq);
        }
        eiointc::enable_irq(irq);
    }

    fn send_ipi(cpu_mask: usize) {
        debug_assert_eq!(cpu_mask, 0);
    }

    #[inline(always)]
    fn get_kernel_stack_top() -> usize {
        let sp: usize;
        unsafe { core::arch::asm!("move {x}, $sp", x = out(reg) sp) };
        sp
    }

    #[inline(always)]
    fn kaddr_to_paddr(kaddr: usize) -> usize {
        kaddr & PA_MASK
    }

    #[inline(always)]
    fn paddr_to_kaddr(paddr: usize) -> usize {
        paddr | DMW1_MASK
    }

    fn dma_direct_paddr(kaddr: usize, len: usize) -> Option<usize> {
        let end = kaddr.checked_add(len)?;
        let direct_map_start = core::ptr::addr_of!(__kernel_start) as usize;
        if len == 0 || kaddr & !PA_MASK != DMW1_MASK || kaddr < direct_map_start || end > *DIRECT_MAP_END {
            return None;
        }
        let paddr = Self::kaddr_to_paddr(kaddr);
        mm::contains_memory_range(paddr, len).then_some(paddr)
    }

    fn map_kernel_addr(_kstart: usize, _pstart: usize, _size: usize, _perm: MapPerm) {}

    unsafe fn unmap_kernel_addr(_kstart: usize, _size: usize) {}

    fn flush_tlb_all() {
        // SAFETY: The caller has completed the page-table update. The barriers
        // publish those writes before invalidating all local translations.
        unsafe {
            core::arch::asm!(
                "dbar 0",
                "invtlb 0x00, $zero, $zero",
                "dbar 0",
                "ibar 0",
                options(nostack, preserves_flags)
            );
        }
    }

    fn flush_tlb_cpu_mask(cpu_mask: usize) {
        assert_eq!(
            cpu_mask, 0,
            "LoongArch targeted remote TLB invalidation is not implemented"
        );
    }

    fn mmio_phys_to_kaddr(paddr: usize, _size: usize) -> usize {
        const DMW0_MASK: usize = 0x8000_0000_0000_0000;
        debug_assert!(paddr < (1usize << 48), "PA {:#x} outside PALEN=48", paddr);
        paddr | DMW0_MASK
    }

    fn uptime() -> Duration {
        Duration::from_micros(Self::get_time_us())
    }

    fn get_time_us() -> u64 {
        csr::rdtime() * 1_000_000 / *STABLE_COUNTER_FREQ_HZ
    }

    fn set_next_time_event_us(interval: u64) {
        let ticks = interval.saturating_mul(*STABLE_COUNTER_FREQ_HZ).saturating_add(999_999) / 1_000_000;
        let tcfg = (ticks.max(1) as usize) << csr::tcfg::INITVAL_SHIFT | csr::tcfg::EN;
        csr::write::<{ csr::num::TCFG }>(tcfg);
    }

    fn scan_device() {
        if let Err(()) = fdt::load_device_tree() {
            crate::kwarn!("loongarch: FDT walk failed; continuing without devices");
        }
    }

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

    #[inline(always)]
    fn get_frame_pointer() -> usize {
        let fp: usize;
        unsafe { core::arch::asm!("move {x}, $r22", x = out(reg) fp) };
        fp
    }

    #[inline(always)]
    unsafe fn frame_info(fp: usize) -> (usize, usize) {
        let p = fp as *const usize;
        unsafe { (*p.sub(1), *p.sub(2)) }
    }

    #[inline(always)]
    fn is_kernel_addr(addr: usize) -> bool {
        // DMW0/1 live in the upper half (bit 63 set); every kernel VA comes
        // out of paddr_to_kaddr, which OR-s in DMW1_MASK.
        (addr as isize) < 0
    }

    fn elf_native_machine() -> u16 {
        abi::EM_LOONGARCH
    }

    fn kmodule_relocation_action(relocation_type: u32) -> SysResult<KModuleRelocationAction> {
        super::kmodule::relocation_action(relocation_type)
    }

    fn apply_kmodule_relocation(
        relocation_type: u32,
        place: &mut [u8],
        value: Option<KModuleRelocationValue>,
    ) -> SysResult<()> {
        super::kmodule::apply_relocation(relocation_type, place, value)
    }

    fn flush_kmodule_icache() {
        super::kmodule::flush_icache();
    }
}
