use crate::arch::riscv::process;
use crate::arch::{Arch, ArchTrait, UserContextTrait};
use crate::kernel::scheduler::current;
use crate::driver;

use super::kernel_switch;
use super::KernelContext;
use super::kernelpagetable;
use super::csr::{Sstatus, SIE, stvec};

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

    fn get_kernel_stack_top() -> usize {
        let sp;
        unsafe {
            core::arch::asm!("mv {}, sp", out(reg) sp);
        }
        sp
    }

    fn scan_device() {
        driver::load_device_tree(unsafe { __riscv_copied_fdt });
    }

    fn kaddr_offset() -> usize {
        unsafe { __riscv_kaddr_offset }
    }

    fn kaddr_to_paddr(kaddr: usize) -> usize {
        kaddr - unsafe { __riscv_kaddr_offset }
    }

    fn paddr_to_kaddr(paddr: usize) -> usize {
        paddr + unsafe { __riscv_kaddr_offset }
    }
}
