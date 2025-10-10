#include <stdio.h>

static const char *BEFORE = "before_rename.txt";
static const char *AFTER  = "after_rename.txt";

int main() {
    FILE *before = fopen(BEFORE, "w");
    if (before == NULL) {
        perror("fopen before");
        return 1;
    }

    if (fprintf(before, "This file will be renamed.\n") < 0) {
        perror("fprintf");
        fclose(before);
        return 1;
    }

    fclose(before);

    rename(BEFORE, AFTER);

    FILE *after = fopen(AFTER, "r");
    if (after == NULL) {
        perror("fopen after");
        return 1;
    }

    char buffer[256];
    int r;
    if ((r = fread(buffer, 1, sizeof(buffer)-1, after)) < 0) {
        perror("fread");
        fclose(after);
        return 1;
    }
    buffer[r] = 0;
    printf("Read from renamed file: %s", buffer);

    fclose(after);

    return 0;
}
