#include <stdio.h>

const char DATA[] = "Hello, World!";

int main() {
    FILE *fp = tmpfile();
    if (fp == NULL) {
        perror("tmpfile");
        return 1;
    }

    size_t written = fwrite(DATA, 1, sizeof(DATA) - 1, fp);
    if (written != sizeof(DATA) - 1) {
        perror("fwrite");
        return 1;
    }

    rewind(fp);
    char buffer[1024];
    ssize_t read = fread(buffer, 1, sizeof(buffer), fp);
    if (read < 0) {
        perror("fread");
        return 1;
    }

    buffer[read] = '\0';
    printf("Read from tempfile: %s\n", buffer);

    return 0;
}
