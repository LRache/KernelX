#if __riscv_xlen == 64
    #define LOAD(dest, n)  ld dest, (n*8)(sp)
    #define STORE(src, n)  sd src , (n*8)(sp)
#else
    #define LOAD(dest, n)  lw dest, (n*4)(sp)
    #define STORE(src, n)  sw src , (n*4)(sp)
#endif

    .global asm_kerneltrap_entry

.align 3
asm_kerneltrap_entry:
    csrw sscratch, sp

    la sp, __ktrap_temp_stack_end
    
    addi sp, sp, -32*__riscv_xlen/8
    
    STORE(ra, 1)
    STORE(gp, 3)
    STORE(tp, 4)
    STORE(t0, 5)
    STORE(t1, 6)
    STORE(t2, 7)
    STORE(s0, 8)
    STORE(s1, 9)
    STORE(a0, 10)
    STORE(a1, 11)
    STORE(a2, 12)
    STORE(a3, 13)
    STORE(a4, 14)
    STORE(a5, 15)
    STORE(a6, 16)
    STORE(a7, 17)
    STORE(s2, 18)
    STORE(s3, 19)
    STORE(s4, 20)
    STORE(s5, 21)
    STORE(s6, 22)
    STORE(s7, 23)
    STORE(s8, 24)
    STORE(s9, 25)
    STORE(s10, 26)
    STORE(s11, 27)
    STORE(t3, 28)
    STORE(t4, 29)
    STORE(t5, 30)
    STORE(t6, 31)
    
    call kerneltrap_handler
    
    LOAD(ra, 1)
    LOAD(gp, 3)
    LOAD(tp, 4)
    LOAD(t0, 5)
    LOAD(t1, 6)
    LOAD(t2, 7)
    LOAD(s0, 8)
    LOAD(s1, 9)
    LOAD(a0, 10)
    LOAD(a1, 11)
    LOAD(a2, 12)
    LOAD(a3, 13)
    LOAD(a4, 14)
    LOAD(a5, 15)
    LOAD(a6, 16)
    LOAD(a7, 17)
    LOAD(s2, 18)
    LOAD(s3, 19)
    LOAD(s4, 20)
    LOAD(s5, 21)
    LOAD(s6, 22)
    LOAD(s7, 23)
    LOAD(s8, 24)
    LOAD(s9, 25)
    LOAD(s10, 26)
    LOAD(s11, 27)
    LOAD(t3, 28)
    LOAD(t4, 29)
    LOAD(t5, 30)
    LOAD(t6, 31)

    addi sp, sp, 32*__riscv_xlen/8

    csrr sp, sscratch

    sret
