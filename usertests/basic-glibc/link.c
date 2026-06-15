#include <errno.h>
#include <stdio.h>
#include <string.h>
#include <unistd.h>

static const char *SRC = "link_src.txt";
static const char *DST = "link_dst.txt";

static int write_text(const char *path, const char *text) {
    FILE *fp = fopen(path, "w");
    if (fp == NULL) {
        perror("fopen");
        return -1;
    }

    if (fputs(text, fp) == EOF) {
        perror("fputs");
        fclose(fp);
        return -1;
    }

    if (fclose(fp) != 0) {
        perror("fclose");
        return -1;
    }

    return 0;
}

static int read_text(const char *path, char *buf, size_t size) {
    FILE *fp = fopen(path, "r");
    size_t nread;

    if (fp == NULL) {
        perror("fopen");
        return -1;
    }

    nread = fread(buf, 1, size - 1, fp);
    if (ferror(fp)) {
        perror("fread");
        fclose(fp);
        return -1;
    }

    buf[nread] = '\0';

    if (fclose(fp) != 0) {
        perror("fclose");
        return -1;
    }

    return 0;
}

int main(void) {
    char buffer[64];

    unlink(SRC);
    unlink(DST);

    if (write_text(SRC, "source") != 0) {
        return 1;
    }
    if (write_text(DST, "dest") != 0) {
        return 1;
    }

    if (link(SRC, DST) != -1) {
        fprintf(stderr, "link unexpectedly succeeded\n");
        return 1;
    }
    if (errno != EEXIST) {
        fprintf(stderr, "link returned errno %d instead of %d\n", errno, EEXIST);
        return 1;
    }

    if (read_text(DST, buffer, sizeof(buffer)) != 0) {
        return 1;
    }
    if (strcmp(buffer, "dest") != 0) {
        fprintf(stderr, "destination content changed: %s\n", buffer);
        return 1;
    }

    if (unlink(DST) != 0) {
        perror("unlink");
        return 1;
    }

    if (access(DST, F_OK) == 0) {
        fprintf(stderr, "destination still exists after unlink\n");
        return 1;
    }
    if (errno != ENOENT) {
        fprintf(stderr, "access returned errno %d instead of %d\n", errno, ENOENT);
        return 1;
    }

    unlink(SRC);
    printf("link duplicate-entry regression passed\n");
    return 0;
}
