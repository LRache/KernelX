
use crate::arch::arch::UserContextTrait;
use crate::kernel::mm::AddrSpace;
use crate::kernel::scheduler::KernelStack;

/// User-space register state saved on every kernel entry.
///
/// Offsets are consumed by the asm in `clib/.../usertrap.S`:
/// `kernel_sp` must stay at byte 256, `kernel_percpu` at 264.
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct UserContext {
    /*  0 */ pub gpr: [usize; 32],
    /* 32 */ pub kernel_sp: usize,
    /* 33 */ pub kernel_percpu: usize, // saved $r21 across trap boundary
    /* 34 */ pub user_pgd: usize,      // PGDL value for user
    /* 35 */ pub kernel_pgd: usize,    // unused; DMW covers kernel
    /* 36 */ pub usertrap_handler: usize,
    /* 37 */ pub fpregs: [u128; 32], // 128-bit to cover LSX ($vr0..$vr31)
    pub fcc: u64,
    pub fcsr: u64,
    pub user_entry: usize,
    pub fpregs_dirty: bool,
}

impl UserContextTrait for UserContext {
    fn new() -> Self {
        UserContext {
            gpr: [0; 32],
            kernel_sp: 0,
            kernel_percpu: 0,
            user_pgd: 0,
            kernel_pgd: 0,
            usertrap_handler: 0,
            fpregs: [0u128; 32],
            fcc: 0,
            fcsr: 0,
            user_entry: 0,
            fpregs_dirty: true,
        }
    }

    fn new_clone(&self) -> Self {
        let mut new_context = self.clone();
        new_context.kernel_sp = 0;
        new_context.kernel_percpu = 0;
        new_context.user_pgd = 0;
        // clone(2) returns 0 to the child; a0 is $r4.
        new_context.gpr[4] = 0;
        new_context
    }

    fn get_user_stack_top(&self) -> usize {
        self.gpr[3] // sp
    }

    fn set_user_stack_top(&mut self, user_stack_top: usize) {
        self.gpr[3] = user_stack_top;
    }

    fn set_kernel_stack_top(&mut self, kernel_stack_top: usize) {
        self.kernel_sp = kernel_stack_top;
    }

    fn set_addrspace(&mut self, addrspace: &AddrSpace) {
        addrspace.with_pagetable(|pagetable| {
            self.user_pgd = pagetable.get_pgd();
        });
    }

    fn set_sigaction_restorer(&mut self, uptr_restorer: usize) -> &mut Self {
        self.gpr[1] = uptr_restorer; // ra
        self
    }

    fn set_arg(&mut self, index: usize, arg: usize) -> &mut Self {
        debug_assert!(index <= 7);
        self.gpr[4 + index] = arg; // a0..a7
        self
    }

    fn restore_from_signal(&mut self, sigcontext: &SigContext) -> &mut Self {
        self.gpr[1..32].copy_from_slice(&sigcontext.gregs);
        self.fpregs.copy_from_slice(&sigcontext.fpregs);
        self.fcc = sigcontext.fcc;
        self.fcsr = sigcontext.fcsr;
        self.user_entry = sigcontext.pc;
        self
    }

    fn get_user_entry(&self) -> usize {
        self.user_entry
    }

    fn set_user_entry(&mut self, entry: usize) -> &mut Self {
        self.user_entry = entry;
        self
    }

    fn skip_syscall_instruction(&mut self) {
        self.user_entry += 4; // `syscall` is 4 bytes
    }

    fn move_back_to_syscall_instruction(&mut self) {
        self.user_entry -= 4;
    }

    fn set_tls(&mut self, tls: usize) {
        self.gpr[2] = tls; // tp
    }

    fn set_syscall_retval(&mut self, retval: usize) {
        self.gpr[4] = retval; // a0
    }
}

unsafe impl Send for UserContext {}
unsafe impl Sync for UserContext {}

#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct KernelContext {
    ra: usize,
    sp: usize,
    fp: usize,        // $r22
    s: [usize; 9],    // $s0..$s8
    a0: usize,        // $r4: first arg to entry
}

impl KernelContext {
    pub fn new(kernel_stack: &KernelStack) -> Self {
        KernelContext {
            ra: super::task::traphandle::return_to_user as usize,
            sp: kernel_stack.get_top(),
            fp: 0,
            s: [0; 9],
            a0: 0,
        }
    }

    pub fn new_idle() -> Self {
        KernelContext {
            ra: 0,
            sp: 0,
            fp: 0,
            s: [0; 9],
            a0: 0,
        }
    }

    pub fn set_entry(&mut self, entry: usize) -> &mut Self {
        self.ra = entry;
        self
    }

    pub fn set_arg0(&mut self, arg: usize) -> &mut Self {
        self.a0 = arg;
        self
    }

    pub fn frame_pointer(&self) -> usize {
        self.fp
    }
}

unsafe impl Send for KernelContext {}
unsafe impl Sync for KernelContext {}

/// Snapshot of user state saved during signal delivery.
#[repr(C)]
#[repr(align(16))]
#[derive(Clone, Copy, Debug)]
pub struct SigContext {
    pub pc: usize,
    pub gregs: [usize; 31], // $r1..$r31
    pub fpregs: [u128; 32],
    pub fcc: u64,
    pub fcsr: u64,
}

impl SigContext {
    pub fn empty() -> Self {
        SigContext {
            pc: 0,
            gregs: [0; 31],
            fpregs: [0u128; 32],
            fcc: 0,
            fcsr: 0,
        }
    }
}

impl From<UserContext> for SigContext {
    fn from(uc: UserContext) -> Self {
        let mut gregs: [usize; 31] = [0; 31];
        gregs.copy_from_slice(&uc.gpr[1..32]);
        SigContext {
            pc: uc.user_entry,
            gregs,
            fpregs: uc.fpregs,
            fcc: uc.fcc,
            fcsr: uc.fcsr,
        }
    }
}
