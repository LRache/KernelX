#ifndef __KERNELX_BOOT_FDT_H__
#define __KERNELX_BOOT_FDT_H__

#include "boot/memory.h"

#ifdef __cplusplus
extern "C" {
#endif

size_t kernelx_fdt_memory_regions(const void *fdt, struct kernelx_mem_region *regions,
                                  size_t capacity);

#ifdef __cplusplus
}
#endif

#endif /* __KERNELX_BOOT_FDT_H__ */
