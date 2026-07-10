#define _GNU_SOURCE

#include <errno.h>
#include <fcntl.h>
#include <stdio.h>
#include <string.h>
#include <sys/stat.h>
#include <unistd.h>

static int expect_eq(const char *lhs_name, long lhs, long rhs) {
    if (lhs != rhs) {
        fprintf(stderr, "%s mismatch: got %ld, want %ld\n", lhs_name, lhs, rhs);
        return -1;
    }
    return 0;
}

int main(void) {
    const char *src_path = "splice_src.txt";
    const char *dst_path = "splice_dst.txt";
    const char *payload = "splice smoke test payload";
    const size_t payload_len = strlen(payload);

    int src = open(src_path, O_CREAT | O_TRUNC | O_RDWR, 0644);
    if (src < 0) {
        perror("open src");
        return 1;
    }

    if (write(src, payload, payload_len) != (ssize_t)payload_len) {
        perror("write src");
        close(src);
        return 1;
    }

    if (lseek(src, 0, SEEK_SET) < 0) {
        perror("lseek src");
        close(src);
        return 1;
    }

    int dst = open(dst_path, O_CREAT | O_TRUNC | O_RDWR, 0644);
    if (dst < 0) {
        perror("open dst");
        close(src);
        return 1;
    }

    int append_dst = open("splice_append_dst.txt", O_CREAT | O_TRUNC | O_RDWR | O_APPEND, 0644);
    if (append_dst < 0) {
        perror("open append dst");
        close(src);
        close(dst);
        return 1;
    }

    int pipefd[2];
    if (pipe(pipefd) < 0) {
        perror("pipe");
        close(src);
        close(dst);
        close(append_dst);
        return 1;
    }

    ssize_t moved = splice(src, NULL, pipefd[1], NULL, payload_len, 0);
    if (moved < 0) {
        perror("splice file->pipe");
        close(pipefd[0]);
        close(pipefd[1]);
        close(src);
        close(dst);
        close(append_dst);
        return 1;
    }
    if (expect_eq("file->pipe bytes", moved, (long)payload_len) < 0) {
        return 1;
    }

    moved = splice(pipefd[0], NULL, dst, NULL, payload_len, 0);
    if (moved < 0) {
        perror("splice pipe->file");
        close(pipefd[0]);
        close(pipefd[1]);
        close(src);
        close(dst);
        close(append_dst);
        return 1;
    }
    if (expect_eq("pipe->file bytes", moved, (long)payload_len) < 0) {
        return 1;
    }

    if (lseek(dst, 0, SEEK_SET) < 0) {
        perror("lseek dst");
        close(pipefd[0]);
        close(pipefd[1]);
        close(src);
        close(dst);
        close(append_dst);
        return 1;
    }

    char buf[128] = {0};
    ssize_t n = read(dst, buf, sizeof(buf) - 1);
    if (n < 0) {
        perror("read dst");
        close(pipefd[0]);
        close(pipefd[1]);
        close(src);
        close(dst);
        close(append_dst);
        return 1;
    }
    if (expect_eq("dst bytes", n, (long)payload_len) < 0) {
        return 1;
    }
    if (memcmp(buf, payload, payload_len) != 0) {
        fprintf(stderr, "dst content mismatch: got '%s'\n", buf);
        close(pipefd[0]);
        close(pipefd[1]);
        close(src);
        close(dst);
        close(append_dst);
        return 1;
    }

    off_t off = 0;
    moved = splice(pipefd[0], &off, dst, NULL, 1, 0);
    if (moved != -1 || errno != ESPIPE) {
        fprintf(stderr, "expected ESPIPE for pipe offset, got ret=%zd errno=%d\n", moved, errno);
        close(pipefd[0]);
        close(pipefd[1]);
        close(src);
        close(dst);
        close(append_dst);
        return 1;
    }

    if (write(pipefd[1], payload, payload_len) != (ssize_t)payload_len) {
        perror("write pipe");
        close(pipefd[0]);
        close(pipefd[1]);
        close(src);
        close(dst);
        close(append_dst);
        return 1;
    }

    errno = 0;
    moved = splice(pipefd[0], NULL, append_dst, NULL, 1, 0);
    if (moved != -1 || errno != EINVAL) {
        fprintf(stderr, "expected EINVAL for O_APPEND output, got ret=%zd errno=%d\n", moved, errno);
        close(pipefd[0]);
        close(pipefd[1]);
        close(src);
        close(dst);
        close(append_dst);
        return 1;
    }

    printf("splice smoke test ok\n");

    close(pipefd[0]);
    close(pipefd[1]);
    close(src);
    close(dst);
    close(append_dst);
    return 0;
}
