use crate::arch::{Arch, ArchTrait, UserContextTrait};
use crate::kernel::scheduler::current;

use super::kernel_switch;
use super::KernelContext;
use super::kernelpagetable;
use super::csr::{Sstatus, SIE, stvec};

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
}
