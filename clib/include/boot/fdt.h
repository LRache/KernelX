#ifndef __KERNELX_BOOT_FDT_H__
#define __KERNELX_BOOT_FDT_H__

#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

uintptr_t kernelx_fdt_memory_top(const void *fdt);

#ifdef __cplusplus
}
#endif

#endif /* __KERNELX_BOOT_FDT_H__ */
