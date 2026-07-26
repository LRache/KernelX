use alloc::boxed::Box;
use alloc::sync::Arc;
use core::time::Duration;
use elf::abi;

use crate::arch::riscv::sbi_driver::{SBIConsoleDriver, SBIKPMU};
use crate::arch::riscv::{csr, load_device_tree, plic, sbi_driver, task};
use crate::arch::{self, Arch, ArchTrait, CloneABI, UserContextTrait};
use crate::driver::chosen;
use crate::kernel::config;
use crate::kernel::errno::SysResult;
use crate::kernel::mm::{self, MapPerm};
use crate::kernel::scheduler::current;
use crate::klib::{InitedCell, SpinLock};
use crate::kmodule::{KModuleRelocationAction, KModuleRelocationValue};
use crate::{driver, kinfo, kwarn};

use super::csr::{SIE, Sstatus, sscratch};
use super::pagetable::kernelpagetable;
use super::sbi_driver::SBIKConsole;
use super::task::context::KernelContext;
use super::{core_count, kernel_switch, time_frequency, try_time_frequency};

unsafe extern "C" {
    static __kernel_start: u8;
    static __riscv_copied_fdt: *const u8;
    static __riscv_kaddr_offset: usize;
}

static DIRECT_MAP_END: InitedCell<usize> = InitedCell::uninit();
static NEXT_MMIO_KADDR: InitedCell<SpinLock<usize>> = InitedCell::uninit();

impl Arch {
    pub fn try_uptime() -> Option<Duration> {
        let freq = try_time_frequency()? as u64;
        if freq == 0 {
            return None;
        }
        Some(Duration::from_micros(
            csr::time::read().saturating_mul(1_000_000) / freq,
        ))
    }
}

fn init_mmio_kaddr(max_memory_kaddr: usize) {
    let direct_map_end = align_up(max_memory_kaddr, arch::PGSIZE);
    assert!(
        direct_map_end <= super::KERNEL_MMIO_START,
        "RISC-V direct map overlaps the dynamic kernel address space"
    );
    DIRECT_MAP_END.init(direct_map_end);
    NEXT_MMIO_KADDR.init(SpinLock::new(super::KERNEL_MMIO_START, "NEXT_MMIO_KADDR"));
}

fn alloc_mmio_kaddr(size: usize) -> usize {
    let mut next = NEXT_MMIO_KADDR.lock();
    let kaddr = *next;
    let new_next = kaddr.checked_add(size).expect("RISC-V MMIO virtual address overflow");
    if new_next > super::KERNEL_MMIO_END {
        panic!("RISC-V MMIO virtual address space exhausted");
    }
    *next = new_next;
    kaddr
}

fn align_up(addr: usize, align: usize) -> usize {
    debug_assert!(align.is_power_of_two());
    (addr + align - 1) & !(align - 1)
}

impl ArchTrait for Arch {
    fn init() {
        init_mmio_kaddr(Self::paddr_to_kaddr(mm::max_memory_end()));
        kernelpagetable::init();
        task::init_kernel_stack_allocator();

        chosen::kconsole::register(Box::new(SBIKConsole));
        chosen::kpmu::register(Arc::new(SBIKPMU));

        driver::register_matched_driver(Arc::new(SBIConsoleDriver));
    }

    fn init_percpu() {
        let mut stack_top = sscratch::read();
        if stack_top == 0 {
            unsafe extern "C" {
                static __ktrap_temp_stack_end: u8;
            }
            stack_top = core::ptr::addr_of!(__ktrap_temp_stack_end) as usize;
        }
        current::processor().set_kernel_trap_stack_top(stack_top);
        task::traphandle::set_stvec_to_kerneltrap_handler();
    }

    fn setup_all_cores(current_core: usize) {
        unsafe extern "C" {
            static __riscv_others_entry: u8;
        }

        kinfo!("Starting other harts...");

        let interrupt_stack_base = mm::page::alloc_contiguous_zero(config::KERNEL_TRAP_STACK_PAGE_COUNT);
        let interrupt_stack_top = interrupt_stack_base + config::KERNEL_TRAP_STACK_PAGE_COUNT * arch::PGSIZE;
        // This allocation is owned by the boot hart for the kernel lifetime.
        current::processor().set_kernel_trap_stack_top(interrupt_stack_top);
        sscratch::write(interrupt_stack_top);

        for hartid in 0..core_count() {
            if hartid != current_core {
                let interrupt_stack_base = mm::page::alloc_contiguous_zero(config::KERNEL_TRAP_STACK_PAGE_COUNT);
                let interrupt_stack_top = interrupt_stack_base + config::KERNEL_TRAP_STACK_PAGE_COUNT * arch::PGSIZE;
                let stack = task::KernelStack::<{ config::SCHEDULER_KSTACK_PAGE_COUNT - 1 }>::new();
                let stack_top = stack.get_top();
                // SAFETY: The bootstrap stack is exclusively owned here, and its top
                // 16 bytes are reserved for the secondary-hart handoff before `main`.
                unsafe {
                    core::ptr::write_volatile(
                        (stack_top - core::mem::size_of::<usize>()) as *mut usize,
                        interrupt_stack_top,
                    );
                }
                if let Err(error) = sbi_driver::hart_start(
                    hartid,
                    Self::kaddr_to_paddr(core::ptr::addr_of!(__riscv_others_entry) as usize),
                    stack_top,
                ) {
                    mm::page::free_contiguous(interrupt_stack_base, config::KERNEL_TRAP_STACK_PAGE_COUNT);
                    kwarn!("Failed to start hart {}: SBI error {}", hartid, error);
                } else {
                    kinfo!("Hart {} started successfully", hartid);
                    // The secondary hart uses both stacks for the lifetime of
                    // the kernel, so the bootstrap stack mapping remains live
                    // and the raw interrupt-stack allocation is not freed.
                    core::mem::forget(stack);
                }
            }
        }
    }

    fn clone_abi() -> CloneABI {
        CloneABI::Backwards
    }

    fn cpu_count() -> usize {
        core_count()
    }

    #[inline(always)]
    fn set_percpu_data(data: usize) {
        unsafe { core::arch::asm!("mv tp, {data}", data = in(reg) data) };
    }

    #[inline(always)]
    fn get_percpu_data() -> usize {
        let data: usize;
        unsafe { core::arch::asm!("mv {data}, tp", data = out(reg) data) };
        data
    }

    fn get_user_pc() -> usize {
        current::tcb().user_context().get_user_entry()
    }

    #[inline(always)]
    fn return_to_user() -> ! {
        task::traphandle::return_to_user()
    }

    #[inline(always)]
    fn kernel_switch(from: *mut KernelContext, to: *mut KernelContext) {
        kernel_switch(from, to);
    }

    fn wait_for_interrupt() {
        unsafe { core::arch::asm!("wfi") };
    }

    fn enable_interrupt() {
        Sstatus::read().set_sie(true).write();
    }

    fn disable_interrupt() {
        Sstatus::read().set_sie(false).write();
    }

    fn enable_timer_interrupt() {
        SIE::read().set_stie(true).write();
    }

    fn enable_software_interrupt() {
        SIE::read().set_ssie(true).write();
    }

    fn enable_device_interrupt(hartid: usize) {
        SIE::read().set_seie(true).write();
        plic::enable_interrupt_for_hart(hartid);
    }

    fn enable_device_interrupt_irq(irq: u32) {
        plic::enable_irq_for_all_harts(irq);
    }

    fn get_kernel_stack_top() -> usize {
        let sp;
        unsafe {
            core::arch::asm!("mv {}, sp", out(reg) sp);
        }
        sp
    }

    fn scan_device() {
        load_device_tree(unsafe { __riscv_copied_fdt }).unwrap();
    }

    fn kaddr_to_paddr(kaddr: usize) -> usize {
        kaddr - unsafe { __riscv_kaddr_offset }
    }

    fn paddr_to_kaddr(paddr: usize) -> usize {
        paddr + unsafe { __riscv_kaddr_offset }
    }

    fn dma_direct_paddr(kaddr: usize, len: usize) -> Option<usize> {
        let end = kaddr.checked_add(len)?;
        let direct_map_start = core::ptr::addr_of!(__kernel_start) as usize;
        if len == 0 || kaddr < direct_map_start || end > *DIRECT_MAP_END {
            return None;
        }
        let paddr = Self::kaddr_to_paddr(kaddr);
        mm::contains_memory_range(paddr, len).then_some(paddr)
    }

    fn map_kernel_addr(kstart: usize, pstart: usize, size: usize, perm: MapPerm) {
        kernelpagetable::map_kernel_addr(kstart, pstart, size, perm);
    }

    unsafe fn unmap_kernel_addr(kstart: usize, size: usize) {
        unsafe { kernelpagetable::unmap_kernel_addr(kstart, size) };
    }

    fn flush_tlb_all() {
        // SAFETY: The caller has completed the page-table update before
        // invoking this function. The fence publishes those writes before
        // invalidating all local address translations.
        unsafe {
            core::arch::asm!(
                "fence rw, rw",
                "sfence.vma zero, zero",
                options(nostack, preserves_flags)
            )
        };

        #[cfg(not(feature = "no-smp"))]
        sbi_driver::remote_sfence_vma_all().unwrap_or_else(|error| panic!("SBI remote SFENCE.VMA failed: {error}"));
    }

    fn flush_tlb_cpu_mask(cpu_mask: usize) {
        if cpu_mask == 0 {
            return;
        }

        debug_assert_eq!(
            cpu_mask
                & 1usize
                    .checked_shl(current::hart_id().try_into().expect("hart ID does not fit in u32"))
                    .expect("hart ID exceeds TLB CPU mask width"),
            0,
            "targeted TLB flush mask contains the current hart"
        );

        // SAFETY: Page-table writes are complete before this function is
        // called. The fence publishes them before remote invalidation.
        unsafe {
            core::arch::asm!("fence rw, rw", options(nostack, preserves_flags));
        }

        #[cfg(not(feature = "no-smp"))]
        sbi_driver::remote_sfence_vma(cpu_mask)
            .unwrap_or_else(|error| panic!("SBI targeted remote SFENCE.VMA failed: {error}"));
    }

    fn send_ipi(cpu_mask: usize) {
        if cpu_mask == 0 {
            return;
        }

        debug_assert_eq!(
            cpu_mask
                & 1usize
                    .checked_shl(current::hart_id().try_into().expect("hart ID does not fit in u32"))
                    .expect("hart ID exceeds IPI CPU mask width"),
            0,
            "wakeup IPI mask contains the current hart"
        );

        #[cfg(not(feature = "no-smp"))]
        sbi_driver::send_ipi(cpu_mask).unwrap_or_else(|error| panic!("SBI send IPI failed: {error}"));
    }

    fn mmio_phys_to_kaddr(paddr: usize, size: usize) -> usize {
        let offset = paddr & arch::PGMASK;
        let pbase = paddr - offset;
        let size = size.checked_add(offset).expect("RISC-V MMIO mapping size overflow");
        let mapped_size = arch::page_count(size) * arch::PGSIZE;
        let kbase = alloc_mmio_kaddr(mapped_size);
        kernelpagetable::map_kernel_addr(kbase, pbase, mapped_size, MapPerm::R | MapPerm::W);
        kbase + offset
    }

    fn uptime() -> Duration {
        Duration::from_micros(Self::get_time_us())
    }

    fn get_time_us() -> u64 {
        let t = csr::time::read();
        let f = time_frequency() as u64;
        t / f * 1_000_000 + (t % f) * 1_000_000 / f
    }

    fn set_next_time_event_us(interval: u64) {
        let ticks = interval.saturating_mul(time_frequency() as u64).saturating_add(999_999) / 1_000_000;
        sbi_driver::set_timer(csr::time::read().saturating_add(ticks.max(1)));
    }

    fn read_volatile<T>(src: *const T) -> T {
        unsafe {
            let v = core::ptr::read_volatile(src);
            core::arch::asm!("fence i, r", options(nostack, preserves_flags));
            v
        }
    }

    fn write_volatile<T>(dst: *mut T, val: T) {
        unsafe {
            core::arch::asm!("fence w, o", options(nostack, preserves_flags));
            core::ptr::write_volatile(dst, val);
        }
    }

    #[inline(always)]
    fn get_frame_pointer() -> usize {
        let fp: usize;
        unsafe { core::arch::asm!("mv {}, fp", out(reg) fp) };
        fp
    }

    #[inline(always)]
    unsafe fn frame_info(fp: usize) -> (usize, usize) {
        let p = fp as *const usize;
        unsafe { (*p.sub(1), *p.sub(2)) }
    }

    #[inline(always)]
    fn is_kernel_addr(addr: usize) -> bool {
        addr >> 63 != 0
    }

    fn elf_native_machine() -> u16 {
        abi::EM_RISCV
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

    fn crc32c(seed: u32, buf: &[u8]) -> u32 {
        super::crc32::crc32c(seed, buf)
    }
}
