#define __NR_rt_sigreturn 139

    .section .text
    .global __vdso_sigreturn_trampoline

__vdso_sigreturn_trampoline:
    li a7, __NR_rt_sigreturn
    ecall
