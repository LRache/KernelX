#ifndef KERNELX_KMODULE_STRING_H
#define KERNELX_KMODULE_STRING_H

#include <stddef.h>

#ifdef __cplusplus
extern "C" {
#endif

int strcmp(const char *s1, const char *s2);
int strncmp(const char *s1, const char *s2, size_t n);
char *strcpy(char *dest, const char *src);
size_t strnlen(const char *s, size_t maxlen);
char *strrchr(const char *s, int ch);
void *memchr(const void *ptr, int ch, size_t n);

#ifdef __cplusplus
}
#endif

#endif
