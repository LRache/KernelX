#ifndef __KXEMU_KLIB_H__
#define __KXEMU_KLIB_H__

#include <stddef.h>

#ifdef __cplusplus
extern "C" {
#endif

int printf(const char *fmt, ...) __attribute__ ((format (printf, 1, 2)));

void *memcpy(void *__restrict dest, const void *__restrict src, size_t n);
void *memset(void *s, int c, size_t n);
int   memcmp(const void *s1, const void *s2, size_t n);

void *malloc(size_t size);
void *realloc(void *ptr, size_t size);
void  free  (void *ptr);

#ifdef __cplusplus
}
#endif

#endif
