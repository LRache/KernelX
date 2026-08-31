#include <limits.h>
#include <stdlib.h>
#include <string.h>

unsigned long strtoul(const char *str, char **endptr, int base) {
    const char *s = str;
    unsigned long value = 0;
    int negative = 0;
    int converted = 0;
    int overflow = 0;

    while (*s == ' ' || *s == '\t' || *s == '\n' || *s == '\v' ||
           *s == '\f' || *s == '\r') {
        s++;
    }

    if (*s == '+' || *s == '-') {
        negative = *s == '-';
        s++;
    }

    if (base != 0 && (base < 2 || base > 36)) {
        if (endptr)
            *endptr = (char *)str;
        return 0;
    }

    if ((base == 0 || base == 16) && s[0] == '0' &&
        (s[1] == 'x' || s[1] == 'X') &&
        ((s[2] >= '0' && s[2] <= '9') ||
         (s[2] >= 'a' && s[2] <= 'f') ||
         (s[2] >= 'A' && s[2] <= 'F'))) {
        base = 16;
        s += 2;
    } else if (base == 0) {
        base = *s == '0' ? 8 : 10;
    }

    for (;; s++) {
        unsigned int digit;
        if (*s >= '0' && *s <= '9')
            digit = (unsigned int)(*s - '0');
        else if (*s >= 'a' && *s <= 'z')
            digit = (unsigned int)(*s - 'a') + 10;
        else if (*s >= 'A' && *s <= 'Z')
            digit = (unsigned int)(*s - 'A') + 10;
        else
            break;

        if (digit >= (unsigned int)base)
            break;
        converted = 1;
        if (value > (ULONG_MAX - digit) / (unsigned int)base) {
            overflow = 1;
        } else if (!overflow) {
            value = value * (unsigned int)base + digit;
        }
    }

    if (endptr)
        *endptr = (char *)(converted ? s : str);
    if (overflow)
        return ULONG_MAX;
    return negative ? 0UL - value : value;
}

static void swap(char *a, char *b, size_t size) {
    while (size--) {
        char tmp = *a;
        *a++ = *b;
        *b++ = tmp;
    }
}

static void _qsort_inner(char *base, size_t left, size_t right, size_t size,
                      int (*compar)(const void *, const void *)) {
    if (left >= right)
        return;

    size_t i = left, j = right;
    char *pivot = base + ((left + right) / 2) * size;

    while (i <= j) {
        while (compar(base + i * size, pivot) < 0) i++;
        while (compar(base + j * size, pivot) > 0) j--;

        if (i <= j) {
            swap(base + i * size, base + j * size, size);
            i++;
            if (j > 0) j--;
        }
    }

    if (j > left)
        _qsort_inner(base, left, j, size, compar);
    if (i < right)
        _qsort_inner(base, i, right, size, compar);
}

void qsort(void *base, size_t n, size_t size,
           int (*compar)(const void *, const void *)) {
    if (n <= 1)
        return;

    _qsort_inner((char *)base, 0, n - 1, size, compar);
}
