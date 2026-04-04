use crate::arch::arch::UserContextTrait;
use crate::kernel::mm::AddrSpace;
use crate::kernel::scheduler::KernelStack;

const USER_STACK_SIZE: usize = 0x8000;

#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct UserContext {
    /*  0 */ pub gpr: [usize; 32],
    /* 32 */ pub kernel_tp: usize,
    /* 33 */ pub kernel_sp: usize,
    /* 34 */ pub user_pgd: usize,
    /* 35 */ pub kernel_pgd: usize,
    /* 36 */ pub usertrap_handler: usize,
    /* 37 */ pub fpregs: [u64; 32], // Floating point registers
    pub fcsr: u64,         // Floating point control and status register
    pub user_entry: usize, // User program entry point
}

impl UserContextTrait for UserContext {
    fn new() -> Self {
        // let kernel_pgd = get_kernel_pgd();

        UserContext {
            gpr: [0; 32],
            kernel_tp: 0,
            kernel_sp: 0,
            user_pgd: 0,
            kernel_pgd: 0,
            usertrap_handler: 0, // usertrap_handler as usize,
            fpregs: [0; 32],
            fcsr: 0,
            user_entry: 0,
        }
    }

    fn new_clone(&self) -> Self {
        let mut new_context = self.clone();
        new_context.kernel_sp = 0; // Reset kernel stack pointer
        new_context.user_pgd = 0; // Reset user address space pointer
        new_context.kernel_tp = 0; // Reset kernel thread pointer

        new_context.gpr[4] = 0; // a0, clone returns 0 to the child process

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
            // self.user_pgd = pagetable.get_pgd();
        });
    }

    fn set_sigaction_restorer(&mut self, uptr_restorer: usize) -> &mut Self {
        self.gpr[1] = uptr_restorer; // ra
        self
    }

    fn set_arg(&mut self, index: usize, arg: usize) -> &mut Self {
        debug_assert!(index <= 7);
        self.gpr[4 + index] = arg; // a0-a7
        self
    }

    fn restore_from_signal(&mut self, sigcontext: &SigContext) -> &mut Self {
        self.gpr[1..32].copy_from_slice(&sigcontext.gregs);
        self.fpregs.copy_from_slice(&sigcontext.fpregs[..32]);
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
        self.user_entry += 4; // Skip syscall instruction
    }

    fn set_tls(&mut self, tls: usize) {
        self.gpr[2] = tls; // tp
    }
}

#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct KernelContext {
    pub ra: usize,
    pub sp: usize,
    pub fp: usize,
    pub s: [usize; 9],
    pub a0: usize,
}

impl KernelContext {
    pub fn new(kernel_stack: &KernelStack) -> Self {
        KernelContext {
            ra: 0, // return_to_user as usize,
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

    pub fn set_entry(&mut self, entry: usize) {
        self.ra = entry;
    }
}

#[repr(C)]
#[repr(align(16))]
#[derive(Clone, Copy, Debug)]
pub struct SigContext {
    pub pc: usize,
    pub gregs: [usize; 31], // General registers
    pub fpregs: [u64; 32],  // Floating point registers
    pub fcsr: u64,
}

impl SigContext {
    pub fn empty() -> Self {
        SigContext {
            pc: 0,
            gregs: [0; 31],
            fpregs: [0; 32],
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
            fcsr: uc.fcsr,
        }
    }
}

unsafe impl Send for UserContext {}
unsafe impl Sync for UserContext {}

unsafe impl Send for KernelContext {}
unsafe impl Sync for KernelContext {}

pub fn new_user(entry_point: usize, user_stack_top: usize, kernel_stack_top: usize, arg: usize) -> UserContext {
    let mut context = UserContext::new();
    context.set_user_entry(entry_point);
    context.set_user_stack_top(user_stack_top);
    context.set_kernel_stack_top(kernel_stack_top);
    context.set_arg(0, arg);
    context
}
