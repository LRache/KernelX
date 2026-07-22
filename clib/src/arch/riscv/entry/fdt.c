#include "arch/riscv/entry.h"
#include "boot/fdt.h"
#include "libfdt.h"

#include <stdint.h>

void *__riscv_copied_fdt;

__init_text
size_t __riscv_load_fdt(const void *fdt, struct kernelx_mem_region **regions) {
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

    *ktop = (*ktop + KERNELX_MEM_REGION_PAGE_SIZE - 1) &
            ~(uintptr_t)(KERNELX_MEM_REGION_PAGE_SIZE - 1);
    *regions = (struct kernelx_mem_region *)*ktop;
    *ktop += KERNELX_MEM_REGION_PAGE_SIZE;

    size_t region_count =
        kernelx_fdt_memory_regions(fdt, *regions, KERNELX_MEM_REGION_CAPACITY);
    if (region_count == 0) {
        __riscv_init_die("no memory node found in FDT\n");
    }

    return region_count;
}
