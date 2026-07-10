#include "arch/riscv/entry.h"
#include "boot/fdt.h"
#include "libfdt.h"

#include <stdint.h>

void *__riscv_copied_fdt;

__init_text
uintptr_t __riscv_load_fdt(const void *fdt) {
    uintptr_t *ktop = (uintptr_t *)__riscv_init_symbol_ktop();
    
    if (fdt_check_header(fdt) != 0) {
        __riscv_init_die("FDT header is invalid.\n");
        return 0;
    }

    uint32_t fdt_size = fdt_totalsize(fdt);
    
    const char *src = (const char *)fdt;
    char *dst = (char *)*ktop;
    for (uint32_t i = 0; i < fdt_size; i++) {
        dst[i] = src[i];
    }

    *__riscv_init_symbol_copied_fdt() = (void *)(dst + *__riscv_init_symbol_kaddr_offset());
    *ktop += fdt_size;

    uintptr_t memory_top = kernelx_fdt_memory_top(fdt);
    if (memory_top == 0) {
        __riscv_init_die("no memory node found in FDT\n");
    }

    return memory_top;
}
