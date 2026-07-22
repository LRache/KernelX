#include <stdint.h>

#include "arch/riscv/entry.h"

__init_data
uintptr_t __riscv_ktop;

uintptr_t __riscv_kaddr_offset;

__init_text
void __riscv_init(uintptr_t hartid, const void *fdt, uintptr_t kaddr_offset)  {
    // Clear BSS
    // Assume BSS in aligned to 4K
    uintptr_t* bss_start = (uintptr_t *)__riscv_init_symbol_bss_start();
    uintptr_t* bss_end   = (uintptr_t *)__riscv_init_symbol_bss_end();
    for (uintptr_t* p = bss_start; p < bss_end; p++) {
        *p = 0;
    }
    
    *__riscv_init_symbol_ktop() = __riscv_init_symbol_kernel_end();
    *__riscv_init_symbol_kaddr_offset() = kaddr_offset;

    struct kernelx_mem_region *mem_regions;
    size_t mem_region_count = __riscv_load_fdt(fdt, &mem_regions);
    uintptr_t satp = __riscv_map_kaddr(kaddr_offset, mem_regions, mem_region_count);

    void *ktop = *__riscv_init_symbol_ktop() + kaddr_offset;
    void *mem_regions_kaddr = (void *)((uintptr_t)mem_regions + kaddr_offset);

    /*
        Return:
        a0: hartid
        a1: bootstrap allocation end
        a2: memory region array
        a3: memory region count
        a4: satp
    */
    asm volatile (
        "mv a0, %0\n"
        "mv a1, %1\n"
        "mv a2, %2\n"
        "mv a3, %3\n"
        "mv a4, %4\n"
        :
        : "r"(hartid), "r"(ktop), "r"(mem_regions_kaddr), "r"(mem_region_count),
          "r"(satp)
        : "a0", "a1", "a2", "a3", "a4"
    );
}

__init_text
static inline void sbi_putchar(char c) {
    asm volatile (
        "li a6, 0\n"
        "li a7, 1\n"
        "mv a0, %0\n"
        "ecall\n"
        :
        : "r"(c)
        : "a0", "a6", "a7"
    );
}

__init_text
static inline void sbi_shutdown() {
    asm volatile (
        "li a6, 0\n"
        "li a7, 8\n"
        "ecall\n"
        :
        :
        : "a0", "a7"
    );
}

__init_text
void __riscv_init_die(const char *reason) {
    const char *msg = "Kernel panic: ";
    for (const char *p = msg; *p != '\0'; p++) {
        sbi_putchar(*p);
    }
    for (const char *p = reason; *p != '\0'; p++) {
        sbi_putchar(*p);
    }
    sbi_shutdown();
    while(1);
}
