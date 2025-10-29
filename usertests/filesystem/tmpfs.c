#include <stdio.h>

const char data[] = "Hello, tmpfs!";

int main() {
    FILE *f;
    f = fopen("/tmp/tmpfile", "w");
    if (f == NULL) {
        perror("Failed to open file");
        return 1;
    }

    fwrite(data, sizeof(char), sizeof(data) - 1, f);

    fclose(f);

    f = fopen("/tmp/tmpfile", "r");
    if (f == NULL) {
        perror("Failed to open file for reading");
        return 1;
    }

    char buffer[50];
    size_t n = fread(buffer, sizeof(char), sizeof(buffer) - 1, f);
    buffer[n] = '\0';
    printf("Read from tmpfs: %s\n", buffer);
    
    return 0;
}
