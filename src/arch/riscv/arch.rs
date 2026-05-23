use core::time::Duration;

use alloc::boxed::Box;
use alloc::sync::Arc;

use crate::arch::riscv::sbi_driver::{SBIConsoleDriver, SBIKPMU};
use crate::arch::riscv::{csr, load_device_tree, plic, sbi_driver, task};
use crate::arch::{self, Arch, ArchTrait, CloneABI, UserContextTrait};
use crate::driver::chosen;
use crate::kernel::config;
use crate::kernel::mm::{MapPerm, page};
use crate::kernel::scheduler::current;
use crate::klib::{InitedCell, SpinLock};
use crate::{driver, kinfo, kwarn};

use super::csr::{SIE, Sstatus, stvec};
use super::pagetable::kernelpagetable;
use super::sbi_driver::SBIKConsole;
use super::task::context::KernelContext;
use super::{core_count, kernel_switch, time_frequency};

unsafe extern "C" {
    static __riscv_copied_fdt: *const u8;
    static __riscv_kaddr_offset: usize;
}

static NEXT_MMIO_KADDR: InitedCell<SpinLock<usize>> = InitedCell::uninit();

fn init_mmio_kaddr(memory_top: usize) {
    NEXT_MMIO_KADDR.init(SpinLock::new(align_up(memory_top, arch::PGSIZE), "NEXT_MMIO_KADDR"));
}

fn alloc_mmio_kaddr(size: usize) -> usize {
    let mut next = NEXT_MMIO_KADDR.lock();
    let kaddr = *next;
    let new_next = kaddr.checked_add(size).expect("RISC-V MMIO virtual address overflow");
    if new_next > super::TRAMPOLINE_BASE {
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
    fn init(memory_top: usize) {
        unsafe extern "C" {
            fn asm_kerneltrap_entry() -> !;
        }
        init_mmio_kaddr(memory_top);
        stvec::write(asm_kerneltrap_entry as *const () as usize);
        kernelpagetable::init();

        chosen::kconsole::register(Box::new(SBIKConsole));
        chosen::kpmu::register(Arc::new(SBIKPMU));

        driver::register_matched_driver(Arc::new(SBIConsoleDriver));
    }

    fn setup_all_cores(current_core: usize) {
        unsafe extern "C" {
            static __riscv_others_entry: u8;
        }

        kinfo!("Starting other harts...");

        for hartid in 0..core_count() {
            if hartid != current_core {
                let stack = page::alloc_contiguous(config::SCHEDULER_KSTACK_PAGE_COUNT);
                if let Err(error) = sbi_driver::hart_start(
                    hartid,
                    core::ptr::addr_of!(__riscv_others_entry) as usize,
                    stack + config::SCHEDULER_KSTACK_PAGE_COUNT * arch::PGSIZE,
                ) {
                    kwarn!("Failed to start hart {}: SBI error {}", hartid, error);
                } else {
                    kinfo!("Hart {} started successfully", hartid);
                }
            }
        }
    }

    fn clone_abi() -> CloneABI {
        CloneABI::Backwards
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

    fn map_kernel_addr(kstart: usize, pstart: usize, size: usize, perm: MapPerm) {
        kernelpagetable::map_kernel_addr(kstart, pstart, size, perm);
    }

    unsafe fn unmap_kernel_addr(kstart: usize, size: usize) {
        unsafe { kernelpagetable::unmap_kernel_addr(kstart, size) };
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
        csr::time::read() * 1000000 / (time_frequency() as u64)
    }

    fn set_next_time_event_us(interval: u64) {
        sbi_driver::set_timer(csr::time::read() + interval);
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
            core::arch::asm!("fence w, i", options(nostack, preserves_flags));
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
}
