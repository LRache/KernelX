#include "arch/riscv/entry.h"
#include <stdint.h>

const unsigned int LEVEL = 2;
const uintptr_t PGSIZE = 4096;
const uintptr_t PMDSIZE = 2 * 1024 * 1024;

enum {
    PTE_V = 1 << 0,
    PTE_R = 1 << 1,
    PTE_W = 1 << 2,
    PTE_X = 1 << 3,
    PTE_G = 1 << 5,
    PTE_A = 1 << 6,
    PTE_D = 1 << 7, 
};

enum {
    ROOT_LEVEL = 0,
    PMD_LEVEL = 1,
    PTE_LEVEL = 2,
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
static inline void map_leaf(
    uintptr_t root,
    uintptr_t kaddr,
    uintptr_t paddr,
    uint8_t flags,
    unsigned int leaf_level
) {
    uintptr_t ppn = get_ppn(root);
    for (unsigned int level = ROOT_LEVEL; level <= leaf_level; level++) {
        uint64_t vpn = (kaddr >> (12 + (LEVEL - level) * 9)) & 0x1ff;
        uintptr_t *pagetable = (uintptr_t *)(ppn << 12);
        uintptr_t *pte = &pagetable[vpn];
        
        if (level == leaf_level) {
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
static inline void map_range(
    uintptr_t root,
    uintptr_t kaddr,
    uintptr_t paddr,
    uintptr_t size,
    uint8_t flags
) {
    uintptr_t end = paddr + size;

    while (paddr < end) {
        uintptr_t remaining = end - paddr;
        if (((kaddr | paddr) & (PMDSIZE - 1)) == 0 && remaining >= PMDSIZE) {
            map_leaf(root, kaddr, paddr, flags, PMD_LEVEL);
            kaddr += PMDSIZE;
            paddr += PMDSIZE;
        } else {
            map_leaf(root, kaddr, paddr, flags, PTE_LEVEL);
            kaddr += PGSIZE;
            paddr += PGSIZE;
        }
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
    
    uintptr_t init_start = (uintptr_t)__riscv_init_symbol_init_start();
    uintptr_t init_end   = (uintptr_t)__riscv_init_symbol_init_end();
    flags = PTE_V | PTE_R | PTE_W | PTE_X | PTE_G | PTE_A | PTE_D;
    map_range(root, init_start, init_start, init_end - init_start, flags);
    map_range(root, init_start + kaddr_offset, init_start, init_end - init_start, flags);

    uintptr_t text_start = (uintptr_t)__riscv_init_symbol_text_start();
    uintptr_t text_end   = (uintptr_t)__riscv_init_symbol_text_end();
    flags = PTE_V | PTE_R | PTE_X | PTE_G | PTE_A | PTE_D;
    map_range(root, text_start + kaddr_offset, text_start, text_end - text_start, flags);
    
    uintptr_t rodata_start = (uintptr_t)__riscv_init_symbol_rodata_start();
    uintptr_t rodata_end   = (uintptr_t)__riscv_init_symbol_rodata_end();
    flags = PTE_V | PTE_R | PTE_G | PTE_A | PTE_D;
    map_range(root, rodata_start + kaddr_offset, rodata_start, rodata_end - rodata_start, flags);
    
    uintptr_t data_start = (uintptr_t)__riscv_init_symbol_data_start();
    memory_top = (memory_top + PGSIZE - 1) & ~(PGSIZE - 1);
    flags = PTE_V | PTE_R | PTE_W | PTE_G | PTE_A | PTE_D;
    map_range(root, data_start + kaddr_offset, data_start, memory_top - data_start, flags);

    uintptr_t satp = (8ULL << 60) | get_ppn(root);
    return satp;
}
