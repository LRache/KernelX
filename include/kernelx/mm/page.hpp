#ifndef __KERNELX_MM_PAGE_H__
#define __KERNELX_MM_PAGE_H__

#include <cstddef>

namespace kernelx::page {
    static constexpr size_t PAGE_SIZE = 4096; // 4KB
    
    void *alloc();
    void free(void *ptr);
}

#endif
