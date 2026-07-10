use crate::kernel::syscall::UserStruct;

#[repr(C)]
#[derive(Clone, Copy, Default, UserStruct)]
pub struct KvmRegs {
    pub pc: usize,
    pub gpr: [usize; 31],
}

#[repr(C)]
#[derive(Clone, Copy, Default, UserStruct)]
pub struct KvmSRegs {
    pub satp: usize,
}

#[repr(C)]
#[derive(Clone, Copy, Default, UserStruct)]
pub struct KvmPageFault {
    pub addr: usize,
    pub access_type: usize,
    pub inst: usize,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct VCpuContext {
    /* Guest context */
    /*  0 */ gpr: [usize; 32],

    /* Host context */
    /* 32 */ kernel_tp: usize, // Kernel Thread Pointer
    /* 33 */ kernel_ra: usize, // Kernel Return Address
    /* 34 */ kernel_sp: usize, // Kernel Stack Pointer
    /* 35 */ kernel_s: [usize; 12], // Kernel Saved Registers (s0-s11)

    /* Guest floating-point context */
    /* 47 */ fpregs: [u64; 33], // Floating point registers and fcsr
}

impl VCpuContext {
    pub fn new() -> Self {
        Self {
            gpr: [0; 32],
            fpregs: [0; 33],
            kernel_tp: 0,
            kernel_ra: 0,
            kernel_sp: 0,
            kernel_s: [0; 12],
        }
    }

    pub fn fpregs_mut(&mut self) -> &mut [u64; 33] {
        &mut self.fpregs
    }

    pub fn regs(&self, pc: usize) -> KvmRegs {
        KvmRegs {
            pc,
            gpr: self.gpr[1..].try_into().unwrap(),
        }
    }

    pub fn set_regs(&mut self, regs: KvmRegs) {
        self.gpr[0] = 0;
        self.gpr[1..].copy_from_slice(&regs.gpr);
    }

    pub fn gpr(&self) -> &[usize; 32] {
        &self.gpr
    }

    pub fn gpr_mut(&mut self) -> &mut [usize; 32] {
        &mut self.gpr
    }
}
