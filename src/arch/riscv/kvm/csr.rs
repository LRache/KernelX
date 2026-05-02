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

#[derive(Clone, Copy)]
pub enum VirtualInterrupt {
    Software = 2,
    Timer = 6,
    External = 10,
}

pub mod hideleg {
    use super::VirtualInterrupt;

    pub struct Hideleg {
        value: usize,
    }

    impl Hideleg {
        pub fn clear() -> Self {
            Self { value: 0 }
        }

        pub fn write(&self) {
            unsafe {
                core::arch::asm!("csrw hideleg, {}", in(reg) self.value);
            }
        }

        pub fn delegate(&mut self, interrupt: VirtualInterrupt) -> &mut Self {
            self.value |= 1 << interrupt as usize;
            self
        }
    }
}

pub mod hvip {
    use super::VirtualInterrupt;

    #[derive(Clone, Copy)]
    pub struct Hvip {
        value: usize,
    }

    impl Hvip {
        pub const fn clear() -> Self {
            Self { value: 0 }
        }

        pub fn read() -> Self {
            let value: usize;
            unsafe {
                core::arch::asm!("csrr {}, hvip", out(reg) value);
            }
            Self { value }
        }

        pub fn write(&self) {
            unsafe {
                core::arch::asm!("csrw hvip, {}", in(reg) self.value);
            }
        }

        pub fn set_pending(&mut self, interrupt: VirtualInterrupt, pending: bool) -> &mut Self {
            let bit = 1 << interrupt as usize;
            if pending {
                self.value |= bit;
            } else {
                self.value &= !bit;
            }
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

pub mod hcounteren {
    const TM: usize = 1 << 1;

    pub struct Hcounteren {
        value: usize,
    }

    impl Hcounteren {
        pub fn read() -> Self {
            let value: usize;
            unsafe {
                core::arch::asm!("csrr {}, hcounteren", out(reg) value);
            }
            Self { value }
        }

        pub fn write(&self) {
            unsafe {
                core::arch::asm!("csrw hcounteren, {}", in(reg) self.value);
            }
        }

        pub fn set_tm(&mut self, enable: bool) -> &mut Self {
            if enable {
                self.value |= TM;
            } else {
                self.value &= !TM;
            }
            self
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

pub mod htval {
    pub fn read() -> usize {
        let value: usize;
        unsafe {
            core::arch::asm!("csrr {}, htval", out(reg) value);
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

pub mod vstimecmp {
    pub fn write(value: usize) {
        unsafe {
            core::arch::asm!("csrw vstimecmp, {}", in(reg) value);
        }
    }
}
