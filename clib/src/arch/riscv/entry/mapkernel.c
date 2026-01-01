#include "arch/riscv/entry.h"
#include <stdint.h>

const unsigned int LEVEL = 2;
const unsigned int PGSIZE = 4096;

enum {
    PTE_V = 1 << 0,
    PTE_R = 1 << 1,
    PTE_W = 1 << 2,
    PTE_X = 1 << 3,
    PTE_G = 1 << 5,
    PTE_A = 1 << 6,
    PTE_D = 1 << 7, 
};

__init_text
static inline void *alloc_page() {
    void **ktop = __riscv_init_symbol_ktop();
    void *page = *ktop;
    *ktop += PGSIZE;
    return page;
}

uintptr_t __riscv_kpgtable_root;

__init_text
static inline uintptr_t get_ppn(uintptr_t paddr) {
    return paddr >> 12;
}

__init_text
static inline void map(uintptr_t root, uintptr_t kaddr, uint64_t paddr, uint8_t flags) {
    uintptr_t ppn = get_ppn(root);
    for (int level = 0; level <= LEVEL; level++) {
        uint64_t vpn = (kaddr >> (12 + (LEVEL - level) * 9)) & 0x1ff;
        uintptr_t *pagetable = (uintptr_t *)(ppn << 12);
        uintptr_t *pte = &pagetable[vpn];
        
        if (level == LEVEL) {
            *pte = (get_ppn(paddr) << 10) | flags;
            return;
        }

        if (!(*pte & PTE_V)) {
            uintptr_t *new_page = alloc_page();
            for (unsigned int i = 0; i < PGSIZE / sizeof(uintptr_t); i++) {
                new_page[i] = 0;
            }
            *pte = (get_ppn((uintptr_t)new_page) << 10) | PTE_V;
        }

        ppn = *pte >> 10;
    }
}

__init_text
uintptr_t __riscv_map_kaddr(uintptr_t kaddr_offset, uintptr_t memory_top) {
    uintptr_t *ktop = (uintptr_t *)__riscv_init_symbol_ktop();
    *ktop = (*ktop + PGSIZE - 1) & ~(PGSIZE - 1);
    
    uintptr_t root = (uintptr_t)alloc_page();
    *__riscv_init_symbol_kpgtable_root() = root + *__riscv_init_symbol_kaddr_offset();

    for (unsigned int i = 0; i < PGSIZE / sizeof(uintptr_t); i++) {
        ((uintptr_t *)root)[i] = 0;
    }

    uint8_t flags;
    flags = PTE_V | PTE_R | PTE_W | PTE_X | PTE_G | PTE_A | PTE_D;
    for (uintptr_t paddr = (uintptr_t)__riscv_init_symbol_init_start(); paddr < (uintptr_t)__riscv_init_symbol_init_end(); paddr += PGSIZE) {
        uintptr_t kaddr = paddr + kaddr_offset;
        map(root, paddr, paddr, flags);
        map(root, kaddr, paddr, flags);
    }

    flags = PTE_V | PTE_R | PTE_X | PTE_G | PTE_A | PTE_D;
    for (uintptr_t paddr = (uintptr_t)__riscv_init_symbol_text_start(); paddr < (uintptr_t)__riscv_init_symbol_text_end(); paddr += PGSIZE) {
        uintptr_t kaddr = paddr + kaddr_offset;
        map(root, kaddr, paddr, flags);
    }

    flags = PTE_V | PTE_R | PTE_W | PTE_G | PTE_A | PTE_D;
    memory_top = (memory_top + PGSIZE - 1) & ~(PGSIZE - 1);
    for (uintptr_t paddr = (uintptr_t)__riscv_init_symbol_text_end(); paddr < memory_top; paddr += PGSIZE) {
        uintptr_t kaddr = paddr + kaddr_offset;
        map(root, kaddr, paddr, flags);
    }

    uintptr_t satp = (8ULL << 60) | get_ppn(root);
    return satp;
}