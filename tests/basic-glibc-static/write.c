#include <stdio.h>

int main() {
    FILE *file = fopen("testwrite.txt", "w");
    if (file == NULL) {
        perror("Failed to open file");
        return 1;
    }

    int r = fprintf(file, "Hello, World!\n");
    printf("fprintf returned: %d\n", r);
    if (r < 0) {
        perror("Failed to write to file");
    }
    
    fclose(file);

    return 0;
}
