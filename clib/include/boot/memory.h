#ifndef __KERNELX_BOOT_MEMORY_H__
#define __KERNELX_BOOT_MEMORY_H__

#include <stddef.h>
#include <stdint.h>

#define KERNELX_MEM_REGION_PAGE_SIZE 4096

struct kernelx_mem_region {
    uintptr_t start;
    uintptr_t end;
};

#define KERNELX_MEM_REGION_CAPACITY \
    (KERNELX_MEM_REGION_PAGE_SIZE / sizeof(struct kernelx_mem_region))

#endif /* __KERNELX_BOOT_MEMORY_H__ */
