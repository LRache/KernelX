    .section .rodata.bootloader
    .align 12
pagetable:
    .dword 0, 0
    .dword (0x80000 << 10) | 0xcf // 2: VA 0x8000_0000 ~ 0xc000_0000 -> PA 0x8000_0000 ~ 0xc000_0000
    .rept 258 - 2 - 1
        .dword 0
    .endr
    .dword (0x80000 << 10) | 0xcf // 258: VA 0xffff_ffc0_8000_0000 ~ 0xffff_ffc0_c000_0000 to 0x8000_0000 ~ 0xc000_0000
    .rept 512 - 258 - 1
        .dword 0
    .endr

#define PAGETABLE_ADDR 0x80201000
#define ENTRY_ADDR     0xffffffc080205000
    
    .global bootloader
    .section .text.bootloader
    .align 12
bootloader:
    li t0, (PAGETABLE_ADDR >> 12) | (8 << 60)
    
    sfence.vma
    csrw satp, t0
    sfence.vma

    li tp, 0

    li t0, ENTRY_ADDR
    jr t0
