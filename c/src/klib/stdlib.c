#include <stdlib.h>
#include <string.h>

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