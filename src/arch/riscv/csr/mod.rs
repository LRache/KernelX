pub mod scause;

mod sstatus;

pub use sstatus::Sstatus;

pub mod sepc {
    pub fn read() -> usize {
        let value: usize;
        unsafe { core::arch::asm!("csrr {}, sepc", out(reg) value); }
        value
    }

    pub fn write(value: usize) {
        unsafe { core::arch::asm!("csrw sepc, {}", in(reg) value); }
    }
}

pub mod stvec {
    pub fn read() -> usize {
        let value: usize;
        unsafe { core::arch::asm!("csrr {}, stvec", out(reg) value); }
        value
    }

    pub fn write(value: usize) {
        unsafe { core::arch::asm!("csrw stvec, {}", in(reg) value); }
    }
}

pub mod stval {
    pub fn read() -> usize {
        let value: usize;
        unsafe { core::arch::asm!("csrr {}, stval", out(reg) value); }
        value
    }

    pub fn write(value: usize) {
        unsafe { core::arch::asm!("csrw stval, {}", in(reg) value); }
    }
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