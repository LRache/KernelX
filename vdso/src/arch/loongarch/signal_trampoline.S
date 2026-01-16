#define __NR_rt_sigreturn 139

    .section .text
    .global __vdso_sigreturn_trampoline
    .type __vdso_sigreturn_trampoline, @function

__vdso_sigreturn_trampoline:
    li.w    $a7, __NR_rt_sigreturn
    syscall 0
