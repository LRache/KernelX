pub struct SIE {
    sie: usize
}

impl SIE {
    pub fn read() -> Self {
        let sie;
        unsafe { core::arch::asm!("csrr {}, sie", out(reg) sie); }
        SIE { sie }
    }

    pub fn write(&self) {
        unsafe { core::arch::asm!("csrw sie, {}", in(reg) self.sie); }
    }

    pub fn set_stie(&mut self, stie: bool) -> &mut Self {
        if stie {
            self.sie |= 1 << 5;
        } else {
            self.sie &= !(1 << 5);
        }
        self
    }

    pub fn set_seie(&mut self, seie: bool) -> &mut Self {
        if seie {
            self.sie |= 1 << 9;
        } else {
            self.sie &= !(1 << 9);
        }
        self
    }
}
