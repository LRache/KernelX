#[derive(PartialEq, Eq)]
pub enum SstatusFs {
    Off,
    Initial,
    Clean,
    Dirty,
}

pub struct Sstatus {
    sstatus: usize,
}

impl Sstatus {
    pub fn read() -> Self {
        let sstatus;
        unsafe { core::arch::asm!("csrr {}, sstatus", out(reg) sstatus); }
        Self {
            sstatus
        }
    }

    pub fn write(&self) {
        unsafe { core::arch::asm!("csrw sstatus, {}", in(reg) self.sstatus); }
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

    pub fn fs(&self) -> SstatusFs {
        match (self.sstatus >> 13) & 0b11 {
            0 => SstatusFs::Off,
            1 => SstatusFs::Initial,
            2 => SstatusFs::Clean,
            3 => SstatusFs::Dirty,
            _ => unreachable!(),
        }
    }

    pub fn set_fs(&mut self, fs: SstatusFs) -> &mut Self {
        self.sstatus &= !(0b11 << 13);
        self.sstatus |= match fs {
            SstatusFs::Off => 0,
            SstatusFs::Initial => 1,
            SstatusFs::Clean => 2,
            SstatusFs::Dirty => 3,
        } << 13;
        self
    }
}
