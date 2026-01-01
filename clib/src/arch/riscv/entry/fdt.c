#include "arch/riscv/entry.h"
#include "libfdt.h"

#include <stdint.h>

void *__riscv_copied_fdt;

__init_text
static inline uint64_t get_memory_top_from_fdt(const void *fdt) {
    int node_offset;

    fdt_for_each_subnode(node_offset, fdt, 0) {
        const char *node_name = fdt_get_name(fdt, node_offset, NULL);
        if (!node_name) continue;

        if (strncmp(node_name, "memory", 6) == 0) {
            int prop_len;
            const uint32_t *prop_val = fdt_getprop(fdt, node_offset, "reg", &prop_len);
            if (!prop_val) continue;
            
            uint64_t base, size;
            if (prop_len >= 16) {
                base = ((uint64_t)fdt32_to_cpu(prop_val[0]) << 32) | fdt32_to_cpu(prop_val[1]);
                size = ((uint64_t)fdt32_to_cpu(prop_val[2]) << 32) | fdt32_to_cpu(prop_val[3]);
            } else if (prop_len >= 8) {
                base = fdt32_to_cpu(prop_val[0]);
                size = fdt32_to_cpu(prop_val[1]);
            } else {
                __riscv_init_die("prop_len is invalid.");
                return 0;
            }
            return base + size;
        }
    }

    __riscv_init_die("Kernel panic: no memory node found in FDT\n");

    return 0;
}

__init_text
uintptr_t __riscv_load_fdt(const void *fdt) {
    uintptr_t *ktop = __riscv_init_symbol_ktop();
    
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

    return get_memory_top_from_fdt(fdt);
}
