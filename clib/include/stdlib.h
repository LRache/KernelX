#ifndef KERNELX_STDLIB_H
#define KERNELX_STDLIB_H

#include <stddef.h>

#ifdef __cplusplus
extern "C" {
#endif

void *malloc(size_t size);
void *calloc(size_t count, size_t size);
void *realloc(void *ptr, size_t size);
void free(void *ptr);
unsigned long strtoul(const char *str, char **endptr, int base);

void qsort(void *base, size_t count, size_t size,
           int (*compare)(const void *, const void *));

#ifdef __cplusplus
}
#endif

#endif
