pub mod hedeleg {
    use crate::arch::riscv::csr::scause::Trap;

    pub struct Hedeleg {
        value: usize,
    }

    impl Hedeleg {
        pub fn clear() -> Self {
            Self { value: 0 }
        }

        pub fn write(&self) {
            unsafe {
                core::arch::asm!("csrw hedeleg, {}", in(reg) self.value);
            }
        }

        pub fn delegate(&mut self, trap: Trap) -> &mut Self {
            self.value |= 1 << trap as usize;
            self
        }
    }
}
