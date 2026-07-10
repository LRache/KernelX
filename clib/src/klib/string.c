#include <string.h>

#include <kmodule/export.h>

int strcmp(const char *s1, const char *s2) {
    while (*s1 && (*s1 == *s2)) {
        s1++;
        s2++;
    }
    return *(unsigned char *)s1 - *(unsigned char *)s2;
}

KMODULE_EXPORT("strcmp", strcmp);

int strncmp(const char *s1, const char *s2, size_t n) {
    while (n && *s1 && (*s1 == *s2)) {
        s1++;
        s2++;
        n--;
    }
    return n ? *(unsigned char *)s1 - *(unsigned char *)s2 : 0;
}

KMODULE_EXPORT("strncmp", strncmp);

char *strcpy(char *dest, const char *src) {
    char *d = dest;
    while ((*d++ = *src++));
    return dest;
}

KMODULE_EXPORT("strcpy", strcpy);

size_t strnlen(const char *s, size_t maxlen) {
    size_t l = 0;
    while (l < maxlen && s[l])
        ++l;
    return l;
}

KMODULE_EXPORT("strnlen", strnlen);

char *strrchr(const char *s, int ch) {
    char c = ch;
    const char *ret = NULL;
    do {
        if(*s == c)
            ret = s;
    } while(*s++);
    return (char *) ret;
}

KMODULE_EXPORT("strrchr", strrchr);

void *memchr(const void *ptr, int ch, size_t n) {
    const char *p = ptr;
    char c = ch;
    while (n--) {
        if (*p != c)
            ++p;
        else
            return (void *) p;
    }
    return NULL;
}

KMODULE_EXPORT("memchr", memchr);
