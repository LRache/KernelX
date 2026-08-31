#ifndef KERNELX_STRING_H
#define KERNELX_STRING_H

#include <stddef.h>

#ifdef __cplusplus
extern "C" {
#endif

void *memcpy(void *dest, const void *src, size_t count);
void *memmove(void *dest, const void *src, size_t count);
void *memset(void *dest, int value, size_t count);
int memcmp(const void *lhs, const void *rhs, size_t count);
void *memchr(const void *ptr, int value, size_t count);

size_t strlen(const char *str);
size_t strnlen(const char *str, size_t count);
int strcmp(const char *lhs, const char *rhs);
int strncmp(const char *lhs, const char *rhs, size_t count);
char *strcpy(char *dest, const char *src);
char *strncpy(char *dest, const char *src, size_t count);
char *strchr(const char *str, int ch);
char *strrchr(const char *str, int ch);

#ifdef __cplusplus
}
#endif

#endif
