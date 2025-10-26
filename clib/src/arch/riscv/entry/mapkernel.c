#include "arch/riscv/entry.h"
#include <stdint.h>

#define LEVEL 2
#define PTE_V (1 << 0)
#define PTE_R (1 << 1)
#define PTE_W (1 << 2)
#define PTE_X (1 << 3)
#define PTE_G (1 << 5)
#define PTE_A (1 << 6)
#define PTE_D (1 << 7)

__init_text
static inline void *alloc_page() {
    uintptr_t *kernel_end = __riscv_init_load_kernel_end();
    void *page = (void *)*kernel_end;
    *kernel_end += PGSIZE;
    return page;
}

extern char __init_start[];
extern char __init_end  [];
extern char __text_start[];
extern char __text_end  [];

uintptr_t __riscv_kpgtable_root;

__init_text
static inline uintptr_t get_ppn(uintptr_t paddr) {
    return paddr >> 12;
}

__init_text
static inline void map(uintptr_t root, uintptr_t kaddr, uint64_t paddr, uint8_t flag) {
    uintptr_t ppn = get_ppn(root);
    for (int level = 0; level <= LEVEL; level++) {
        uint64_t vpn = (kaddr >> (12 + (LEVEL - level) * 9)) & 0x1ff;
        uintptr_t *pagetable = (uintptr_t *)(ppn << 12);
        uintptr_t *pte = &pagetable[vpn];
        
        if (level == LEVEL) {
            *pte = (get_ppn(paddr) << 10) | flag;
            return;
        }

        if (!(*pte & PTE_V)) {
            void *new_page = alloc_page();
            for (unsigned int i = 0; i < PGSIZE / sizeof(uintptr_t); i++) {
                ((uintptr_t *)new_page)[i] = 0;
            }
            *pte = (get_ppn((uintptr_t)new_page) << 10) | PTE_V;
        }

        ppn = *pte >> 10;
    }
}

__init_text
uintptr_t __riscv_map_kaddr(uintptr_t kaddr_offset, uintptr_t memory_top) {
    uintptr_t *kernel_end = (uintptr_t *)__riscv_init_load_kernel_end();
    *kernel_end = (*kernel_end + PGSIZE - 1) & ~(PGSIZE - 1);
    
    uintptr_t root = (uintptr_t)alloc_page();
    *__riscv_init_load_kpgtable_root() = root + *__riscv_init_load_kaddr_offset();

    for (unsigned int i = 0; i < PGSIZE / sizeof(uintptr_t); i++) {
        ((uintptr_t *)root)[i] = 0;
    }

    for (uintptr_t kaddr = (uintptr_t)__init_start; kaddr < (uintptr_t)__init_end; kaddr += PGSIZE) {
        map(root, kaddr - kaddr_offset, kaddr - kaddr_offset, PTE_V | PTE_R | PTE_W | PTE_X | PTE_G | PTE_A | PTE_D);
        map(root, kaddr, kaddr - kaddr_offset, PTE_V | PTE_R | PTE_W | PTE_X | PTE_G | PTE_A | PTE_D);
    }

    for (uintptr_t kaddr = (uintptr_t)__text_start; kaddr < (uintptr_t)__text_end; kaddr += PGSIZE) {
        map(root, kaddr, kaddr - kaddr_offset, PTE_V | PTE_R | PTE_X | PTE_G | PTE_A | PTE_D);
    }

    memory_top = (memory_top + PGSIZE - 1) & ~(PGSIZE - 1);
    for (uintptr_t paddr = (uintptr_t)__text_end - kaddr_offset; paddr < memory_top; paddr += PGSIZE) {
        map(root, paddr + kaddr_offset, paddr, PTE_V | PTE_R | PTE_W | PTE_G | PTE_A | PTE_D);
    }

    uintptr_t satp = (8ULL << 60) | get_ppn(root);
    return satp;
}