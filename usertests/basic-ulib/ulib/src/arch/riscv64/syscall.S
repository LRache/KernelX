    .global __syscall
    .type __syscall, @function
    .text

# uintptr_t __syscall(uintptr_t number, uintptr_t arg1, uintptr_t arg2, uintptr_t arg3, uintptr_t arg4);
__syscall:
    mv a7, a0
    mv a0, a1
    mv a1, a2
    mv a2, a3
    mv a3, a4

    ecall

    ret
