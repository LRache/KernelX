    .section .bss.bootloader
    .align 12
pagetable:
    .space 4096

#define PAGETABLE_ADDR 0x80201000
    
.global bootloader

.section .text.bootloader
.align 2
bootloader:
    # la t0, pagetable
    li t0, PAGETABLE_ADDR
    
    // map 0x80000000 ~ 0xc0000000 DIRECTLY
    li t1, 0x80000
    slli t1, t1, 10
    ori t1, t1, 0xf
    sd t1, (2 * 8)(t0)
    
    // map 0x80000000 ~ 0xc0000000 to 0xffffffff80000000
    li t2, 510 * 8
    add t0, t0, t2
    sd t1, 0x0(t0) 

    li t0, PAGETABLE_ADDR

    srli t0, t0, 12
    li t1, 8 << 60
    or t0, t0, t1
    csrw satp, t0
    sfence.vma

    li t0, 0xffffffff80205000
    jr t0
