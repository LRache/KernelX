#include <stdint.h>

#include "arch/riscv/entry.h"

__init_data
uintptr_t __riscv_ktop;

uintptr_t __riscv_kaddr_offset;

struct riscv_init_result {
    uintptr_t hartid;
    uintptr_t bootstrap_end;
    uintptr_t mem_regions;
    uintptr_t mem_region_count;
    uintptr_t satp;
};

__init_data
struct riscv_init_result __riscv_init_result;

__init_text
struct riscv_init_result *__riscv_init(uintptr_t hartid, const void *fdt,
                                       uintptr_t kaddr_offset) {
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

    struct riscv_init_result *result = __riscv_init_symbol_init_result();
    result->hartid = hartid;
    result->bootstrap_end = (uintptr_t)ktop;
    result->mem_regions = (uintptr_t)mem_regions_kaddr;
    result->mem_region_count = mem_region_count;
    result->satp = satp;
    return result;
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
