#[inline(always)]
pub fn set_percpu_data(ptr: usize) {
    unsafe {
        core::arch::asm!("mv tp, {}", in(reg) ptr as usize);
    }
}

#[inline(always)]
pub fn get_percpu_data() -> usize {
    let ptr: usize;
    unsafe {
        core::arch::asm!("mv {}, tp", out(reg) ptr);
    }
    ptr
}

pub fn get_user_pc() -> usize {
    let sepc: usize;
    unsafe {
        core::arch::asm!("csrr {}, sepc", out(reg) sepc);
    }
    sepc
}
