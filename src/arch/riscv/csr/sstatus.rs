pub struct Sstatus {
    sstatus: usize,
}

impl Sstatus {
    pub fn read() -> Self {
        let sstatus;
        unsafe {
            core::arch::asm!("csrr {}, sstatus", out(reg) sstatus);
        }
        Self { sstatus }
    }

    pub fn write(&self) {
        unsafe {
            core::arch::asm!("csrw sstatus, {}", in(reg) self.sstatus);
        }
    }

    pub fn set_spie(&mut self, enable: bool) -> &mut Self {
        if enable {
            self.sstatus |= 1 << 5;
        } else {
            self.sstatus &= !(1 << 5);
        }
        self
    }

    pub fn set_sie(&mut self, enable: bool) -> &mut Self {
        if enable {
            self.sstatus |= 1 << 1;
        } else {
            self.sstatus &= !(1 << 1);
        }
        self
    }

    pub fn set_spp(&mut self, user: bool) -> &mut Self {
        if user {
            self.sstatus &= !(1 << 8);
        } else {
            self.sstatus |= 1 << 8;
        }
        self
    }
}
