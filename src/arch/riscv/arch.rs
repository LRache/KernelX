use core::time::Duration;

use alloc::sync::Arc;

use crate::arch::riscv::fdt::core_count;
use crate::arch::riscv::{csr, load_device_tree, plic, process, sbi_driver};
use crate::arch::riscv::sbi_driver::{SBIConsoleDriver, SBIKPMU};
use crate::arch::{self, Arch, ArchTrait, UserContextTrait};
use crate::kernel::config;
use crate::kernel::scheduler::current;
use crate::kernel::mm::{MapPerm, page};
use crate::driver::chosen;
use crate::{driver, kinfo};

use super::KernelContext;
use super::pagetable::kernelpagetable;
use super::csr::{Sstatus, SIE, stvec};
use super::time_frequency;
use super::kernel_switch;
use super::sbi_driver::SBIKConsole;

unsafe extern "C" {
    static __riscv_copied_fdt: *const u8;
    static __riscv_kaddr_offset: usize;
}

impl ArchTrait for Arch {
    fn init() {
        unsafe extern "C" {
            fn asm_kerneltrap_entry() -> !;
        }
        stvec::write(asm_kerneltrap_entry as usize);
        kernelpagetable::init();

        chosen::kconsole::register(&SBIKConsole);
        chosen::kpmu::register(&SBIKPMU);

        driver::register_matched_driver(Arc::new(SBIConsoleDriver));
    }

    fn setup_all_cores(current_core: usize) {
        unsafe extern "C" {
            static __riscv_others_entry: u8;
        }

        kinfo!("Starting other harts...");
        
        plic::enable_interrupt_for_all_harts();

        for hartid in 0..core_count() {
            if hartid != current_core {
                let stack = page::alloc_contiguous(config::SCHEDULER_KSTACK_PAGE_COUNT);
                sbi_driver::hart_start(
                    hartid, 
                    unsafe { &__riscv_others_entry } as *const u8 as usize, 
                    stack + config::SCHEDULER_KSTACK_PAGE_COUNT * arch::PGSIZE
                );
            }
        }
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
        process::traphandle::return_to_user();
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

    fn enable_device_interrupt() {
        SIE::read().set_seie(true).write();
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

    fn uptime() -> Duration {
        Duration::from_micros(Self::get_time_us())
    }

    fn get_time_us() -> u64 {
        csr::time::read() * 1000000 / (time_frequency() as u64)
    }

    fn set_next_time_event_us(interval: u64) {
        sbi_driver::set_timer(csr::time::read() + interval);
    }
}
