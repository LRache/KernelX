#define PTE_FLAG 0xcf
    
    .section .rodata.bootloader
    .align 12
pagetable:
    .dword 0
    .dword (0x040000 << 10) | PTE_FLAG // 001: VA 0x4000_0000 ~ 0x8000_0000 -> PA 0x4000_0000 ~ 0x8000_4000
    .dword (0x080000 << 10) | PTE_FLAG // 002: VA 0x8000_0000 ~ 0x0c0000000 -> PA 0x8000_0000 ~ 0x0c0004000
    .dword (0x0c0000 << 10) | PTE_FLAG // 003: VA 0xc000_0000 ~ 0x100000000 -> PA 0xc000_0000 ~ 0x1000004000
    .dword (0x100000 << 10) | PTE_FLAG // 004: VA 0x1000_0000 ~ 0x140000000 -> PA 0x1000_0000 ~ 0x1400004000
    .rept 257 - 4 - 1
        .dword 0
    .endr
    .dword (0x040000 << 10) | PTE_FLAG // 257: VA 0xffff_ffc0_4000_0000 ~ 0xffff_fff0_8000_0000 to 0x040000000 ~ 0x080000000
    .dword (0x080000 << 10) | PTE_FLAG // 258: VA 0xffff_ffc0_8000_0000 ~ 0xffff_fff0_c000_0000 to 0x080000000 ~ 0x0c0000000
    .dword (0x0c0000 << 10) | PTE_FLAG // 259: VA 0xffff_ffc0_c000_0000 ~ 0xffff_fff1_0000_0000 to 0x0c0000000 ~ 0x100000000
    .dword (0x100000 << 10) | PTE_FLAG // 260: VA 0xffff_ffc1_0000_0000 ~ 0xffff_fff1_4000_0000 to 0x100000000 ~ 0x140000000
    .rept 512 - 260 - 1
        .dword 0
    .endr

#define PAGETABLE_ADDR (0x40080000 + 0x1000)
#define ENTRY_ADDR     (0xffffffc040080000 + 0x5000)
    
    .global bootloader

    .section .text.bootloader
    .align 2
bootloader:    
    li t0, (PAGETABLE_ADDR >> 12) | (8 << 60)

    sfence.vma
    csrw satp, t0
    sfence.vma

    li t0, ENTRY_ADDR
    jr t0
