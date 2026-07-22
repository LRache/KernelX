#include "arch/loongarch/entry.h"
#include "boot/fdt.h"
#include "libfdt.h"

#include <stdint.h>

#define LOONGARCH_DMW1_MASK UINT64_C(0x9000000000000000)
#define LOONGARCH_FDT_BASE_PA UINT64_C(0x100000)
#define LOONGARCH_FALLBACK_MEMORY_TOP UINT64_C(0x10000000)

__init_text
size_t __la_load_mem_regions(struct kernelx_mem_region *regions) {
    const void *fdt = (const void *)(LOONGARCH_DMW1_MASK | LOONGARCH_FDT_BASE_PA);
    size_t count = 0;

    if (fdt_check_header(fdt) == 0) {
        count = kernelx_fdt_memory_regions(fdt, regions, KERNELX_MEM_REGION_CAPACITY);
    }

    if (count == 0) {
        regions[0] = (struct kernelx_mem_region){
            .start = 0,
            .end = LOONGARCH_FALLBACK_MEMORY_TOP,
        };
        count = 1;
    }

    return count;
}
