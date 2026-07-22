#ifndef __KERNELX_ARCH_LOONGARCH_ENTRY_H__
#define __KERNELX_ARCH_LOONGARCH_ENTRY_H__

#include "boot/memory.h"

#define __init_text __attribute__((section(".text.init")))
#define __init_data __attribute__((section(".data.init")))

size_t __la_load_mem_regions(struct kernelx_mem_region *regions);

#endif /* __KERNELX_ARCH_LOONGARCH_ENTRY_H__ */
