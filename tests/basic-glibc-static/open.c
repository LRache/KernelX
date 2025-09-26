#include <stdio.h>

int main() {
    FILE *file = fopen("testopen.txt", "w");
    if (file == NULL) {
        perror("Failed to open file");
        return 1;
    }

    fprintf(file, "Hello, World!\n");
    fclose(file);

    // file = fopen("testfile.txt", "r");
    // if (file == NULL) {
    //     perror("Failed to open file");
    //     return 1;
    // }

    // char buffer[256];
    // if (fgets(buffer, sizeof(buffer), file) != NULL) {
    //     printf("Read from file: %s", buffer);
    // } else {
    //     perror("Failed to read from file");
    // }
    // fclose(file);

    return 0;
}
