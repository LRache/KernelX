//! User / kernel / signal context definitions for LoongArch64.
//!
//! Register layout follows the LoongArch LP64D psABI:
//!   $r0 = zero, $r1 = ra, $r2 = tp, $r3 = sp, $r4..$r11 = a0..a7,
//!   $r21 = kernel-reserved (we use it for per-CPU data, analogous to
//!   RISC-V's `tp`), $r22 = fp.
//!
//! For Phase 1 we only need the shape to be correct so that code in
//! `src/kernel/**` and `src/arch/loongarch/arch.rs` type-checks. Real trap
//! save / restore will land in Phase 4/5.

use crate::arch::arch::UserContextTrait;
use crate::kernel::mm::AddrSpace;
use crate::kernel::scheduler::KernelStack;

/// User-space register + bookkeeping context saved on every kernel entry.
///
/// The exact field layout is not yet frozen — ASM will consult `#[repr(C)]`
/// offsets in Phase 4, at which point `kernel_sp`/`kernel_pgd`/
/// `usertrap_handler` must stay at stable offsets. For now the layout only
/// needs to be a super-set of what the generic `UserContextTrait` API asks
/// for.
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct UserContext {
    /*  0 */ pub gpr: [usize; 32],
    /* 32 */ pub kernel_sp: usize,
    /* 33 */ pub kernel_percpu: usize, // saved copy of $r21 (per-CPU pointer)
    /* 34 */ pub user_pgd: usize,      // PGDL value for user space
    /* 35 */ pub kernel_pgd: usize,    // PGDL value for kernel (mostly DMW, unused)
    /* 36 */ pub usertrap_handler: usize,
    /* 37 */ pub fpregs: [u64; 32], // $f0..$f31
    pub fcc: u64,                   // condition-flag regs packed
    pub fcsr: u64,                  // floating control/status
    pub user_entry: usize,          // ERA on next `ertn`
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
            fpregs: [0; 32],
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
        // clone(2) returns 0 to the child — a0 is $r4.
        new_context.gpr[4] = 0;
        new_context
    }

    fn get_user_stack_top(&self) -> usize {
        self.gpr[3] // sp == $r3
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
        self.gpr[1] = uptr_restorer; // ra == $r1
        self
    }

    fn set_arg(&mut self, index: usize, arg: usize) -> &mut Self {
        debug_assert!(index <= 7);
        self.gpr[4 + index] = arg; // a0..a7 == $r4..$r11
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
        // LoongArch `syscall` is a 4-byte instruction.
        self.user_entry += 4;
    }

    fn move_back_to_syscall_instruction(&mut self) {
        self.user_entry -= 4;
    }

    fn set_tls(&mut self, tls: usize) {
        self.gpr[2] = tls; // tp == $r2 per psABI
    }

    fn set_syscall_retval(&mut self, retval: usize) {
        self.gpr[4] = retval; // a0 == $r4
    }
}

unsafe impl Send for UserContext {}
unsafe impl Sync for UserContext {}

/// Callee-saved kernel context for `kernel_switch`.
///
/// LoongArch LP64 has $s0..$s8 (9 regs) callee-saved plus $fp and $ra/$sp,
/// matching what we need to preserve across a cooperative switch. Keep $ra
/// at offset 0 so the eventual asm stub is trivial (`st.d $ra, $from, 0`).
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct KernelContext {
    ra: usize,
    sp: usize,
    fp: usize,        // $r22
    s: [usize; 9],    // $s0..$s8 == $r23..$r31
    a0: usize,        // $r4: first arg to thread entry
}

impl KernelContext {
    pub fn new(kernel_stack: &KernelStack) -> Self {
        KernelContext {
            // Phase 5 will swap this for the real return_to_user entry point.
            ra: 0,
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

/// Snapshot of user state saved during signal delivery. Layout kept small
/// and deterministic so a future vDSO `rt_sigreturn` can reconstruct it.
#[repr(C)]
#[repr(align(16))]
#[derive(Clone, Copy, Debug)]
pub struct SigContext {
    pub pc: usize,
    pub gregs: [usize; 31], // $r1..$r31
    pub fpregs: [u64; 32],  // $f0..$f31
    pub fcc: u64,
    pub fcsr: u64,
}

impl SigContext {
    pub fn empty() -> Self {
        SigContext {
            pc: 0,
            gregs: [0; 31],
            fpregs: [0; 32],
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
