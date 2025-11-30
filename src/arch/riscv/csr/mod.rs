pub mod scause;

mod sstatus;
mod sie;

pub use sstatus::Sstatus;
pub use sie::SIE;

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
}

pub mod sscratch {
    pub fn write(value: usize) {
        unsafe { core::arch::asm!("csrw sscratch, {}", in(reg) value); }
    }
}

pub mod time {
    pub fn read() -> u64 {
        let value: usize;
        unsafe { core::arch::asm!("csrr {}, time", out(reg) value); }
        value as u64
    }
}
