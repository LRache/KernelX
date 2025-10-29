    .section .text
    .global _start
_start:
    ld a0, 0(sp)    # Load argc from stack top
    addi a1, sp, 8  # argv pointer

    addi t0, a0, 2  # t0 = argc + 2
    slli t0, t0, 3  # t0 = (argc + 2) * 8
    add a2, sp, t0  # envp pointer
    
    tail __call_main