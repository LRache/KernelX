#include "arch/riscv/entry.h"
#include "libfdt.h"

#include <stdint.h>

void *__riscv_copied_fdt;

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
                __riscv_init_die();
                return 0;
            }
            return base + size;
        }
    }

    __riscv_init_die();

    return 0;
}

__attribute__((section(".text.init")))
uintptr_t __riscv_load_fdt(const void *fdt) {
    uintptr_t *kernel_end = __riscv_init_load_kernel_end();
    
    if (fdt_check_header(fdt) != 0) {
        __riscv_init_die();
        return 0;
    }

    uint32_t fdt_size = fdt_totalsize(fdt);
    
    const char *src = (const char *)fdt;
    char *dst = (char *)*kernel_end;
    for (uint32_t i = 0; i < fdt_size; i++) {
        dst[i] = src[i];
    }

    *__riscv_init_load_copied_fdt() = (void *)(dst + *__riscv_init_load_kaddr_offset());
    *kernel_end += fdt_size;

    return get_memory_top_from_fdt(fdt);
}
