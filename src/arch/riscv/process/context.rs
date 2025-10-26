use crate::arch::arch::UserContextTrait;
use crate::arch::riscv::pagetable::get_kernel_satp;
use crate::arch::riscv::process::traphandle::{usertrap_handler, return_to_user};
use crate::kernel::mm::AddrSpace;
use crate::kernel::task::KernelStack;

#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct UserContext {
    /*  0 */ pub gpr: [usize; 32],
    /* 32 */ pub kernel_tp: usize,    
    /* 33 */ pub kernel_sp: usize,
    /* 34 */ pub user_satp: usize,
    /* 35 */ pub kernel_satp: usize,
    /* 36 */ pub usertrap_handler: usize,
    pub user_entry: usize, // User program entry point
}

impl UserContextTrait for UserContext {
    fn new() -> Self {
        let kernel_satp = get_kernel_satp();
        
        UserContext {
            gpr: [0; 32],
            kernel_tp: 0,
            kernel_sp: 0,
            user_satp: 0,
            kernel_satp,
            usertrap_handler: usertrap_handler as usize,
            user_entry: 0,
        }
    }

    fn new_clone(&self) -> Self {
        let mut new_context = self.clone();
        new_context.kernel_sp = 0; // Reset kernel stack pointer
        new_context.user_satp = 0; // Reset user address space pointer
        new_context.kernel_tp = 0; // Reset kernel thread pointer

        new_context.gpr[10] = 0; // clone returns 0 to the child process
        
        new_context
    }

    fn get_user_stack_top(&self) -> usize {
        self.gpr[2] // sp
    }

    fn set_user_stack_top(&mut self, user_stack_top: usize) {
        self.gpr[2] = user_stack_top;
    }

    fn set_kernel_stack_top(&mut self, kernel_stack_top: usize) {
        self.kernel_sp = kernel_stack_top;
    }

    fn set_addrspace(&mut self, addrspace: &AddrSpace) {
        addrspace.with_pagetable(|pagetable| {
            self.user_satp = pagetable.get_satp();
        });
    }

    fn set_sigaction_restorer(&mut self, uptr_restorer: usize) -> &mut Self {
        self.gpr[1] = uptr_restorer; // ra
        self
    }

    fn restore_from_signal(&mut self, sigcontext: &SigContext) -> &mut Self {
        self.gpr[1..32].copy_from_slice(&sigcontext.gregs);
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
        self.user_entry += 4; // Skip ecall instruction
    }
}

#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct KernelContext {
    pub ra: usize,
    pub sp: usize,
    pub s : [usize; 12],
    pub a0: usize,
}
 
impl KernelContext {
    pub fn new(kernel_stack: &KernelStack) -> Self {
        KernelContext {
            ra: return_to_user as usize,
            sp: kernel_stack.get_top(),
            s: [0; 12],
            a0: 0,
        }
    }
    
    pub fn new_idle() -> Self {
        KernelContext {
            ra: 0,
            sp: 0,
            s : [0; 12],
            a0: 0,
        }
    }
}

#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct SigContext {
    pub pc: usize,
    pub gregs:  [usize; 31], // General registers
    pub fpregs: [u64; 66]    // Floating point registers
}

impl SigContext {
    pub fn empty() -> Self {
        SigContext {
            pc: 0,
            gregs: [0; 31],
            fpregs: [0; 66],
        }
    }
}

impl Into<SigContext> for UserContext {
    fn into(self) -> SigContext {
        let mut gregs: [usize; 31] = [0; 31];
        gregs.copy_from_slice(&self.gpr[1..32]);
        SigContext {
            pc: self.user_entry,
            gregs,
            fpregs: [0; 66], // Placeholder, actual FPU state handling needed
        }
    }
}

unsafe impl Send for UserContext {}
unsafe impl Sync for UserContext {}

unsafe impl Send for KernelContext {}
unsafe impl Sync for KernelContext {}
