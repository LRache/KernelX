#include <stdio.h>
#include <unistd.h>

int puts(const char *s) {
    size_t len = 0;
    while (s[len] != '\0') {
        len++;
    }
    write(1, s, len);
    write(1, "\n", 1);
    return len + 1; // Return number of characters written including newline
}
