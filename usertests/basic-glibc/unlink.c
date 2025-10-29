#include <stdio.h>
#include <unistd.h>

const char *TO_UNLINK = "to_unlink.txt";

int main() {
    FILE *fp = fopen(TO_UNLINK, "r");
    if (fp == NULL) {
        perror("fopen");
        return 1;
    }

    char buffer[256];
    int r;
    if ((r = fread(buffer, 1, sizeof(buffer), fp)) < 0) {
        perror("fread");
        fclose(fp);
        return 1;
    }
    buffer[r] = 0;

    printf("Read from file: %s", buffer);

    fclose(fp);

    if (unlink(TO_UNLINK) == -1) {
        perror("unlink");
        return 1;
    }

    return 0;
}
