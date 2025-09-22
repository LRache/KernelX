use crate::arch::riscv::csr::{csrr_sepc, csrr_tp, csrw_tp};
use crate::kernel::scheduler::Processor;

#[inline(always)]
pub fn current_processor() -> &'static mut Processor<'static> {
    let ptr = csrr_tp() as *mut Processor;
    unsafe {
        &mut *ptr
    }
}

#[inline(always)]
pub fn set_current_processor(ptr: *mut Processor) {
    csrw_tp(ptr as usize);
}

pub fn get_user_pc() -> usize {
    csrr_sepc()
}
