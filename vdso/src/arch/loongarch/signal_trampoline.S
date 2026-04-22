// LoongArch rt_sigreturn vDSO trampoline.
// Syscall number comes from the Linux asm-generic table (rt_sigreturn = 139),
// which LoongArch inherits.
#define __NR_rt_sigreturn 139

    .section .text
    .global __vdso_sigreturn_trampoline

__vdso_sigreturn_trampoline:
    li.w    $a7, __NR_rt_sigreturn
    syscall 0
