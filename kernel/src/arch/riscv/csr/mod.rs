pub mod scause;

pub fn csrr_tp() -> usize {
    let value: usize;
    unsafe {
        core::arch::asm!("mv {}, tp", out(reg) value);
    }
    value
}

pub fn csrw_tp(value: usize) {
    unsafe {
        core::arch::asm!("mv tp, {}", in(reg) value);
    }
}

pub fn csrr_sepc() -> usize {
    let value: usize;
    unsafe {
        core::arch::asm!("csrr {}, sepc", out(reg) value);
    }
    value
}

pub fn csrw_sepc(value: usize) {
    unsafe {
        core::arch::asm!("csrw sepc, {}", in(reg) value);
    }
}

pub fn csrw_stvec(value: usize) {
    unsafe {
        core::arch::asm!("csrw stvec, {}", in(reg) value);
    }
}

pub fn csrr_stval() -> usize {
    let value: usize;
    unsafe {
        core::arch::asm!("csrr {}, stval", out(reg) value);
    }
    value
}

pub fn csrw_sscratch(value: usize) {
    unsafe {
        core::arch::asm!("csrw sscratch, {}", in(reg) value);
    }
}

pub fn csrr_scause() -> usize {
    let value: usize;
    unsafe {
        core::arch::asm!("csrr {}, scause", out(reg) value);
    }
    value
}