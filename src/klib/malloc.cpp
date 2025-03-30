#include <kernelx/klib.h>
#include <kernelx/sync/spinlock.hpp>
#include <tlsf.h>

static tlsf_t tlsf;
static kernelx::sync::SpinLock lock;

void heap_init() {
    extern char __heap_start[];
    extern char __heap_end[];
    tlsf = tlsf_create_with_pool(__heap_start, __heap_end - __heap_start);

    printf("Heap size: %ld\n", __heap_end - __heap_start);
}

void *malloc(size_t size) {
    lock.lock();
    void *ptr = tlsf_malloc(tlsf, size);
    lock.unlock();
    return ptr;
}

void *realloc(void *ptr, size_t size) {
    lock.lock();
    void *new_ptr = tlsf_realloc(tlsf, ptr, size);
    lock.unlock();
    return new_ptr;
}

void free(void *ptr) {
    lock.lock();
    tlsf_free(tlsf, ptr);
    lock.unlock();
}
