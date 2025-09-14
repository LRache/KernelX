#include "tlsf.h"
#include <stddef.h>
#include <stdlib.h>
#include <string.h>

static tlsf_t tlsf = NULL;

void init_heap(void *start, size_t size) {
    tlsf = tlsf_create_with_pool(start, size);
}

void* malloc(size_t size) {
    return tlsf_malloc(tlsf, size);
}

void* malloc_aligned(size_t align, size_t size) {
    if (align <= 8) [[clang::likely]] {
        return tlsf_malloc(tlsf, size);
    } else {
        return tlsf_memalign(tlsf, align, size);
    }
}

void *calloc(size_t count, size_t size) {
    void *ptr = tlsf_malloc(tlsf, count * size);
    if (ptr) {
        memset(ptr, 0, count * size);
    }
    return ptr;
}

void free(void* ptr) {
    tlsf_free(tlsf, ptr);
}
