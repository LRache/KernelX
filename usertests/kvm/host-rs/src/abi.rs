use num_enum::TryFromPrimitive;

#[repr(usize)]
#[derive(Clone, Copy)]
pub enum KvmDeviceIoctl {
    CreateVcpu = 1,
    MapArea = 2,
}

impl KvmDeviceIoctl {
    pub fn request(self) -> libc::c_ulong {
        self as libc::c_ulong
    }
}

#[repr(usize)]
#[derive(Clone, Copy)]
pub enum KvmVcpuIoctl {
    Run = 1,
    GetRegs = 2,
    SetRegs = 3,
    GetSregs = 4,
    GetPageFault = 5,
    SetInterruptPending = 6,
    ClearInterruptPending = 7,
}

impl KvmVcpuIoctl {
    pub fn request(self) -> libc::c_ulong {
        self as libc::c_ulong
    }
}

#[repr(usize)]
#[derive(Clone, Copy, TryFromPrimitive)]
pub enum KvmExitReason {
    MemoryFault = 1,
    Timer = 2,
    SbiCall = 16,
}

#[repr(usize)]
#[derive(Clone, Copy)]
pub enum KvmInterruptKind {
    Hardware = 2,
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct KvmMapArea {
    pub addr: usize,
    pub length: usize,
    pub mapped_addr: usize,
}

#[repr(usize)]
#[derive(Clone, Copy)]
pub enum KvmReg {
    Pc = 0,
    A0 = 10,
    A1 = 11,
    A6 = 16,
    A7 = 17,
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct KvmRegs {
    regs: [usize; 32],
}

impl KvmRegs {
    pub fn get(&self, reg: KvmReg) -> usize {
        self.regs[reg as usize]
    }

    pub fn set(&mut self, reg: KvmReg, value: usize) {
        self.regs[reg as usize] = value;
    }

    pub fn get_index(&self, index: u8) -> usize {
        if index == 0 || index as usize >= self.regs.len() {
            0
        } else {
            self.regs[index as usize]
        }
    }

    pub fn set_index(&mut self, index: u8, value: usize) {
        if index != 0 && (index as usize) < self.regs.len() {
            self.regs[index as usize] = value;
        }
    }
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct KvmSRegs {
    pub satp: usize,
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct KvmPageFault {
    pub addr: usize,
    pub access_type: usize,
    pub inst: usize,
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct KvmInterrupt {
    pub kind: usize,
    pub irq: usize,
}

impl KvmInterrupt {
    pub fn new(kind: KvmInterruptKind, irq: usize) -> Self {
        Self {
            kind: kind as usize,
            irq,
        }
    }
}
