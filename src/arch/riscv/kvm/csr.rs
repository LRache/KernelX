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

pub mod hstatus {
    #[derive(Clone, Copy)]
    pub enum HstatusSpv {
        Hypervisor = 0,
        Virtual = 1,
    }

    pub struct Hstatus {
        value: usize,
    }

    impl Hstatus {
        pub fn read() -> Self {
            let value: usize;
            unsafe {
                core::arch::asm!("csrr {}, hstatus", out(reg) value);
            }
            Self { value }
        }

        pub fn write(&self) {
            unsafe {
                core::arch::asm!("csrw hstatus, {}", in(reg) self.value);
            }
        }

        pub fn spv(&self) -> HstatusSpv {
            if (self.value & (1 << 7)) != 0 {
                HstatusSpv::Virtual
            } else {
                HstatusSpv::Hypervisor
            }
        }

        pub fn set_spv(&mut self, spv: HstatusSpv) -> &mut Self {
            self.value &= !(1 << 7);
            self.value |= (spv as usize) << 7;
            self
        }
    }
}

pub mod hgatp {
    pub fn write(pagetable: usize) {
        unsafe {
            core::arch::asm!("csrw hgatp, {}", in(reg) pagetable);
        }
    }
}

pub mod htinst {
    pub fn read() -> usize {
        let value: usize;
        unsafe {
            core::arch::asm!("csrr {}, htinst", out(reg) value);
        }
        value
    }
}

pub mod vsatp {
    pub fn read() -> usize {
        let value: usize;
        unsafe {
            core::arch::asm!("csrr {}, vsatp", out(reg) value);
        }
        value
    }
    pub fn write(pagetable: usize) {
        unsafe {
            core::arch::asm!("csrw vsatp, {}", in(reg) pagetable);
        }
    }
}
