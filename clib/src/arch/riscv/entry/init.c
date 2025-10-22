#include <stdint.h>

#include "arch/riscv/entry.h"

extern char __kernel_end[];
extern char __bss_start[];
extern char __bss_end  [];

__attribute__((section(".data.init")))
uintptr_t __riscv_kernel_end;

uintptr_t __riscv_kaddr_offset;

__attribute__((section(".text.init")))
void __riscv_init(uintptr_t hartid, const void *fdt, uintptr_t kaddr_offset)  {
    // Clear BSS
    // Assume BSS in aligned to 4K
    uintptr_t bss_start = (uintptr_t)__bss_start - kaddr_offset;
    uintptr_t bss_end   = (uintptr_t)__bss_end   - kaddr_offset;
    for (uintptr_t p = bss_start; p < bss_end; p++) {
        *((char *)p) = 0;
    }
    
    *__riscv_init_load_kernel_end() = (uintptr_t)__kernel_end - kaddr_offset;
    *__riscv_init_load_kaddr_offset() = kaddr_offset;

    uintptr_t memory_top = __riscv_load_fdt(fdt);
    uintptr_t satp = __riscv_map_kaddr(kaddr_offset, memory_top);

    uintptr_t kernel_end = *__riscv_init_load_kernel_end() + kaddr_offset;

    /*
        Return:
        a0: hartid
        a1: heap_start
        a2: satp
    */
    asm volatile (
        "mv a0, %0\n"
        "mv a1, %1\n"
        "mv a2, %2\n"
        :
        : "r"(hartid), "r"(kernel_end), "r"(satp)
        : "a0", "a1", "a2"
    );
}

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

void __riscv_init_die() {
    const char *msg = "Kernel panic: fatal error occurred, shutting down...\n";
    for (const char *p = msg; *p != '\0'; p++) {
        sbi_putchar(*p);
    }
    sbi_shutdown();
    while(1);
}
