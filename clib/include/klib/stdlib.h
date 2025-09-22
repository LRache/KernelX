#ifndef __KLIB_STDIO_H__
#define __KLIB_STDIO_H__

#include <stddef.h>

#ifdef __cplusplus
extern "C" {
#endif

void *malloc(size_t size);
void free(void *ptr);

#ifdef __cplusplus
}
#endif

#endif
