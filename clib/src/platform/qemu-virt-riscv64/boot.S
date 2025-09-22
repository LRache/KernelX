    .section .rodata.bootloader
    .align 12
pagetable:
    .dword 0, 0
    .dword (0x80000 << 10) | 0xcf // MAP 0x80000000 ~ 0xc0000000 DIRECTLY
    .rept 512 - 3 - 2
        .dword 0
    .endr
    .dword (0x80000 << 10) | 0xcf // MAP 0xffffffff80000000 ~ 0xffffffffc0000000 to 0x80000000 ~ 0xc0000000

#define PAGETABLE_ADDR 0x80201000
    
    .global bootloader
    .section .text.bootloader
    .align 12
bootloader:
    li t0, (PAGETABLE_ADDR >> 12) | (8 << 60)
    
    sfence.vma
    csrw satp, t0
    sfence.vma

    li t0, 0xffffffff80205000
    jr t0
