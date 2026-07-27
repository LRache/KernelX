pub mod scause;

mod sie;
mod sstatus;

pub use sie::SIE;
pub use sstatus::*;

pub mod sepc {
    pub fn read() -> usize {
        let value: usize;
        unsafe {
            core::arch::asm!("csrr {}, sepc", out(reg) value);
        }
        value
    }

    pub fn write(value: usize) {
        unsafe {
            core::arch::asm!("csrw sepc, {}", in(reg) value);
        }
    }
}

pub mod stvec {
    pub fn write(value: usize) {
        unsafe {
            core::arch::asm!("csrw stvec, {}", in(reg) value);
        }
    }
}

pub mod stval {
    pub fn read() -> usize {
        let value: usize;
        unsafe {
            core::arch::asm!("csrr {}, stval", out(reg) value);
        }
        value
    }
}

pub mod sscratch {
    pub fn read() -> usize {
        let value: usize;
        // SAFETY: Reading `sscratch` has no memory-safety side effects.
        unsafe {
            core::arch::asm!("csrr {}, sscratch", out(reg) value);
        }
        value
    }

    pub fn write(value: usize) {
        unsafe {
            core::arch::asm!("csrw sscratch, {}", in(reg) value);
        }
    }
}

pub mod sip {
    /// Clear the supervisor software interrupt pending bit (SSIP).
    pub fn clear_ssoft() {
        unsafe {
            core::arch::asm!("csrc sip, {}", in(reg) 1usize << 1);
        }
    }
}

pub mod time {
    pub fn read() -> u64 {
        let value: usize;
        unsafe {
            core::arch::asm!("csrr {}, time", out(reg) value);
        }
        value as u64
    }
}
