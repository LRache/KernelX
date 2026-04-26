#include "arch/riscv/asmtrap.h"
    
    .global asm_kvm_guest_trap_entry
    .global asm_kvm_guest_trap_return
    .type asm_kvm_guest_trap_entry, @function
    .type asm_kvm_guest_trap_return, @function

    .align 3
asm_kvm_guest_trap_entry:
    csrrw a0, sscratch, a0
    
    /* Save the guest registers */
    STORE(ra, 1)
    STORE(sp, 2)
    STORE(gp, 3)
    STORE(tp, 4)
    STORE(t0, 5)
    STORE(t1, 6)
    STORE(t2, 7)
    STORE(s0, 8)
    STORE(s1, 9)
    # STORE(a0, 10)  # a0 is used to save the real a0, so we save it later
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

    /* Load host registers */
    LOAD(tp, 32)
    LOAD(ra, 33)
    LOAD(sp, 34)
    LOAD(s0, 35)
    LOAD(s1, 36)
    LOAD(s2, 37)
    LOAD(s3, 38)
    LOAD(s4, 39)
    LOAD(s5, 40)
    LOAD(s6, 41)
    LOAD(s7, 42)
    LOAD(s8, 43)
    LOAD(s9, 44)
    LOAD(s10, 45)
    LOAD(s11, 46)

    csrr t0, sscratch
    STORE(t0, 10)  # save the real a0

    ret

    .align 3
asm_kvm_guest_trap_return:
    /* Save host registers */
    STORE(tp, 32)
    STORE(ra, 33)
    STORE(sp, 34)
    STORE(s0, 35)
    STORE(s1, 36)
    STORE(s2, 37)
    STORE(s3, 38)
    STORE(s4, 39)
    STORE(s5, 40)
    STORE(s6, 41)
    STORE(s7, 42)
    STORE(s8, 43)
    STORE(s9, 44)
    STORE(s10, 45)
    STORE(s11, 46)

    /* Load guest registers */
    LOAD(ra, 1)
    LOAD(sp, 2)
    LOAD(gp, 3)
    LOAD(tp, 4)
    LOAD(t0, 5)
    LOAD(t1, 6)
    LOAD(t2, 7)
    LOAD(s0, 8)
    LOAD(s1, 9)
    # LOAD(a0, 10)  # a0 is used to save the real
    # a0, so we restore it later
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

    LOAD(a0, 10)  # restore the real a0

    hfence.gvma

    sret
