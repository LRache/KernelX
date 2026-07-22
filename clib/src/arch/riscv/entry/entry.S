    .global _entry
    .type _entry, @function
    .section .text.entry

_entry:
    auipc t0, 0
1:  auipc   t1, %got_pcrel_hi(_entry)
    ld      t1, %pcrel_lo(1b)(t1)
    sub t1, t1, t0 // t1 = KADDR_OFFSET
    mv tp, t1

    // la __stack_end
1:  auipc   sp, %got_pcrel_hi(__stack_end)
    ld      sp, %pcrel_lo(1b)(sp)
    andi sp, sp, -16
    sub sp, sp, t1
    
    mv a2, t1 // a2 = KADDR_OFFSET
    call __riscv_init

    // __riscv_init returns a physical pointer to struct riscv_init_result.
    ld a4, 32(a0) // satp
    ld a3, 24(a0) // memory region count
    ld a2, 16(a0) // memory region array
    ld a1, 8(a0)  // bootstrap allocation end
    ld a0, 0(a0)  // hartid
    
    li tp, 0

    # la sp, __stack_end
1:  auipc   sp, %got_pcrel_hi(__stack_end)
    ld      sp, %pcrel_lo(1b)(sp)
    andi sp, sp, -16

    # la t0, main
1:  auipc   t0, %got_pcrel_hi(main)
    ld      t0, %pcrel_lo(1b)(t0)
    
    sfence.vma
    csrw satp, a4
    sfence.vma
    csrw sscratch, zero

    /*
        We can't use la after setting satp
    */
    
    jalr t0

.spin:
    wfi
    j .spin

    .size _entry, . - _entry

    .global __riscv_others_entry
    .type __riscv_others_entry, @function
__riscv_others_entry:
    auipc t0, 0
1:  auipc   t1, %got_pcrel_hi(__riscv_others_entry)
    ld      t1, %pcrel_lo(1b)(t1)
    sub t1, t1, t0 // t1 = KADDR_OFFSET
    mv tp, t1

    mv s0, a0 // hartid
    mv sp, a1 // stack pointer

    // Set up the satp for this core
    jal __riscv_init_symbol_kpgtable_root
    ld a0, 0(a0)
    sub a0, a0, tp
    li t0, (8 << 60) // SV39
    srli a0, a0, 12
    or a3, t0, a0

    la t0, main

    sfence.vma
    csrw satp, a3
    sfence.vma

    // The bootstrap stack top carries this hart's kernel interrupt stack top.
    ld t1, -8(sp)
    csrw sscratch, t1
    addi sp, sp, -16

    /*
        We can't use la after setting satp
    */

    mv a0, s0 // hartid
    jalr t0

    .size __riscv_others_entry, . - __riscv_others_entry
