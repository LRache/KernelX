use crate::arch::riscv::kernelpagetable::get_kernel_satp;
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
}

impl UserContext {
    pub fn new() -> Self {
        let kernel_satp = get_kernel_satp();
        
        UserContext {
            gpr: [0; 32],
            kernel_tp: 0,
            kernel_sp: 0,
            user_satp: 0,
            kernel_satp,
            usertrap_handler: usertrap_handler as usize,
        }
    }

    pub fn new_clone(&self) -> Self {
        let mut new_context = self.clone();
        new_context.kernel_sp = 0; // Reset kernel stack pointer
        new_context.user_satp = 0; // Reset user address space pointer
        new_context.kernel_tp = 0; // Reset kernel thread pointer

        new_context.gpr[10] = 0; // clone returns 0 to the child process
        
        new_context
    }

    pub fn set_user_stack_top(&mut self, user_stack_top: usize) {
        self.gpr[2] = user_stack_top;
    }

    pub fn set_kernel_stack_top(&mut self, kernel_stack_top: usize) {
        self.kernel_sp = kernel_stack_top;
    }

    pub fn set_addrspace(&mut self, addrspace: &AddrSpace) {
        addrspace.with_pagetable(|pagetable| {
            self.user_satp = pagetable.get_satp();
        });
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

unsafe impl Send for UserContext {}
unsafe impl Sync for UserContext {}

unsafe impl Send for KernelContext {}
unsafe impl Sync for KernelContext {}
