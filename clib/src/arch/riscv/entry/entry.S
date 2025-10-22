    .global _entry
    .section .text.entry

_entry:
    auipc t0, 0
1:  auipc   t1, %got_pcrel_hi(_entry)
    ld      t1, %pcrel_lo(1b)(t1)
    sub t1, t1, t0 // t1 = KADDR_OFFSET
    mv tp, t1
    
    la sp, __stack_end
    andi sp, sp, -16
    sub sp, sp, t1
    
    mv a2, t1 // a2 = KADDR_OFFSET
    call __riscv_init
    
    li tp, 0

    la sp, __stack_end
    la t0, main
    
    sfence.vma
    csrw satp, a2
    sfence.vma

    /*
        We can't use la after setting satp
    */
    
    jalr t0

.spin:
    wfi
    j .spin