use crate::kernel::syscall::UserStruct;

#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct KvmRegs {
    pub pc: usize,
    pub ra: usize,
    pub sp: usize,
    pub gp: usize,
    pub tp: usize,
    pub t0: usize,
    pub t1: usize,
    pub t2: usize,
    pub s0: usize,
    pub s1: usize,
    pub a0: usize,
    pub a1: usize,
    pub a2: usize,
    pub a3: usize,
    pub a4: usize,
    pub a5: usize,
    pub a6: usize,
    pub a7: usize,
    pub s2: usize,
    pub s3: usize,
    pub s4: usize,
    pub s5: usize,
    pub s6: usize,
    pub s7: usize,
    pub s8: usize,
    pub s9: usize,
    pub s10: usize,
    pub s11: usize,
    pub t3: usize,
    pub t4: usize,
    pub t5: usize,
    pub t6: usize,
}

impl UserStruct for KvmRegs {}

#[repr(C)]
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
            ra: self.gpr[1],
            sp: self.gpr[2],
            gp: self.gpr[3],
            tp: self.gpr[4],
            t0: self.gpr[5],
            t1: self.gpr[6],
            t2: self.gpr[7],
            s0: self.gpr[8],
            s1: self.gpr[9],
            a0: self.gpr[10],
            a1: self.gpr[11],
            a2: self.gpr[12],
            a3: self.gpr[13],
            a4: self.gpr[14],
            a5: self.gpr[15],
            a6: self.gpr[16],
            a7: self.gpr[17],
            s2: self.gpr[18],
            s3: self.gpr[19],
            s4: self.gpr[20],
            s5: self.gpr[21],
            s6: self.gpr[22],
            s7: self.gpr[23],
            s8: self.gpr[24],
            s9: self.gpr[25],
            s10: self.gpr[26],
            s11: self.gpr[27],
            t3: self.gpr[28],
            t4: self.gpr[29],
            t5: self.gpr[30],
            t6: self.gpr[31],
        }
    }

    pub fn set_regs(&mut self, regs: KvmRegs) {
        self.gpr[0] = 0;
        self.gpr[1] = regs.ra;
        self.gpr[2] = regs.sp;
        self.gpr[3] = regs.gp;
        self.gpr[4] = regs.tp;
        self.gpr[5] = regs.t0;
        self.gpr[6] = regs.t1;
        self.gpr[7] = regs.t2;
        self.gpr[8] = regs.s0;
        self.gpr[9] = regs.s1;
        self.gpr[10] = regs.a0;
        self.gpr[11] = regs.a1;
        self.gpr[12] = regs.a2;
        self.gpr[13] = regs.a3;
        self.gpr[14] = regs.a4;
        self.gpr[15] = regs.a5;
        self.gpr[16] = regs.a6;
        self.gpr[17] = regs.a7;
        self.gpr[18] = regs.s2;
        self.gpr[19] = regs.s3;
        self.gpr[20] = regs.s4;
        self.gpr[21] = regs.s5;
        self.gpr[22] = regs.s6;
        self.gpr[23] = regs.s7;
        self.gpr[24] = regs.s8;
        self.gpr[25] = regs.s9;
        self.gpr[26] = regs.s10;
        self.gpr[27] = regs.s11;
        self.gpr[28] = regs.t3;
        self.gpr[29] = regs.t4;
        self.gpr[30] = regs.t5;
        self.gpr[31] = regs.t6;
    }
}
