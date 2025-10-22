#include <string.h>

int strcmp(const char *s1, const char *s2) {
    while (*s1 && (*s1 == *s2)) {
        s1++;
        s2++;
    }
    return *(unsigned char *)s1 - *(unsigned char *)s2;
}

int strncmp(const char *s1, const char *s2, size_t n) {
    while (n && *s1 && (*s1 == *s2)) {
        s1++;
        s2++;
        n--;
    }
    return n ? *(unsigned char *)s1 - *(unsigned char *)s2 : 0;
}

char *strcpy(char *dest, const char *src) {
    char *d = dest;
    while ((*d++ = *src++));
    return dest;
}

size_t strnlen(const char *s, size_t maxlen) {
    size_t l = 0;
    while (l < maxlen && s[l])
        ++l;
    return l;
}

char *strrchr(const char *s, int ch) {
    char c = ch;
    const char *ret = NULL;
    do {
        if(*s == c)
            ret = s;
    } while(*s++);
    return (char *) ret;
}

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
