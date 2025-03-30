#include <kernelx/klib.h>

void *memcpy(void *dest, const void *src, size_t n) {
    return __builtin_memcpy(dest, src, n);
}

void *memset(void *s, int c, size_t n) {
    return __builtin_memset(s, c, n);
}

int memcmp(const void *s1, const void *s2, size_t n) {
    return __builtin_memcmp(s1, s2, n);
}
