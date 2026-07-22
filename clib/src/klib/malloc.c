#include "tlsf.h"
#include <kmodule/export.h>
#include <stdint.h>
#include <stddef.h>
#include <string.h>

static tlsf_t tlsf = NULL;

extern void *kernel_heap_alloc(size_t align, size_t size);
extern void kernel_heap_free(void *ptr);

void init_heap(void *start, size_t size) {
    tlsf = tlsf_create_with_pool(start, size);
}

int add_heap_pool(void *start, size_t size) {
    return tlsf_add_pool(tlsf, start, size) != NULL;
}

void *try_malloc_aligned(size_t align, size_t size) {
    if (__builtin_expect(align <= sizeof(void*), 1)) {
        return tlsf_malloc(tlsf, size);
    } else {
        return tlsf_memalign(tlsf, align, size);
    }
}

void free_heap_ptr(void *ptr) {
    tlsf_free(tlsf, ptr);
}

void *malloc(size_t size) {
    return kernel_heap_alloc(sizeof(void*), size);
}

KMODULE_EXPORT("malloc", malloc);

void *malloc_aligned(size_t align, size_t size) {
    return kernel_heap_alloc(align, size);
}

void *calloc(size_t count, size_t size) {
    if (size != 0 && count > SIZE_MAX / size) {
        return NULL;
    }
    void *ptr = malloc(count * size);
    if (ptr) {
        memset(ptr, 0, count * size);
    }
    return ptr;
}

void free(void *ptr) {
    kernel_heap_free(ptr);
}

KMODULE_EXPORT("free", free);
