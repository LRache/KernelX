#include "arch/loongarch/entry.h"
#include "boot/fdt.h"
#include "libfdt.h"

#include <stdint.h>

#define LOONGARCH_DMW1_MASK UINT64_C(0x9000000000000000)
#define LOONGARCH_FDT_BASE_PA UINT64_C(0x100000)
#define LOONGARCH_FALLBACK_MEMORY_TOP UINT64_C(0x10000000)

__init_text
uintptr_t __la_fdt_memory_top(void) {
    const void *fdt = (const void *)(LOONGARCH_DMW1_MASK | LOONGARCH_FDT_BASE_PA);
    uintptr_t memory_top = LOONGARCH_FALLBACK_MEMORY_TOP;

    if (fdt_check_header(fdt) == 0) {
        uintptr_t parsed_top = kernelx_fdt_memory_top(fdt);
        if (parsed_top != 0) {
            memory_top = parsed_top;
        }
    }

    return memory_top | LOONGARCH_DMW1_MASK;
}
