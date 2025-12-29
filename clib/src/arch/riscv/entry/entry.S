    .global _entry
    .global __riscv_others_entry
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
    
    li tp, 0

    # la sp, __stack_end
1:  auipc   sp, %got_pcrel_hi(__stack_end)
    ld      sp, %pcrel_lo(1b)(sp)
    andi sp, sp, -16

    # la t0, main
1:  auipc   t0, %got_pcrel_hi(main)
    ld      t0, %pcrel_lo(1b)(t0)
    
    sfence.vma
    csrw satp, a3
    sfence.vma

    /*
        We can't use la after setting satp
    */
    
    jalr t0

.spin:
    wfi
    j .spin

__riscv_others_entry:
    auipc t0, 0
    auipc   t1, %got_pcrel_hi(__riscv_others_entry)
    ld      t1, %pcrel_lo(1b)(t1)
    sub t1, t1, t0 // t1 = KADDR_OFFSET
    mv tp, t1

    mv s0, a0 // hartid
    mv sp, a1 // stack pointer

    // Set up the satp for this core
    jal __riscv_init_load_kpgtable_root
    li t0, (8 << 60) // SV39
    slli a0, a0, 12
    or a3, t0, a0

    la t0, kentry

    sfence.vma
    csrw satp, a0
    sfence.vma

    /*
        We can't use la after setting satp
    */

    mv a0, s0 // hartid
    jalr t0
