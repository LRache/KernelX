#define _GNU_SOURCE

#include <errno.h>
#include <fcntl.h>
#include <limits.h>
#include <signal.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/resource.h>
#include <sys/sendfile.h>
#include <unistd.h>

enum {
    CHUNK_SIZE = 1024,
    PIPE_CHUNK_SIZE = 4096,
    DATA_SIZE = 8192,
    SHORT_WRITE_SIZE = 1536,
};

static int write_all(int fd, const void *buf, size_t len) {
    const unsigned char *bytes = buf;
    size_t done = 0;
    while (done < len) {
        ssize_t n = write(fd, bytes + done, len - done);
        if (n < 0 && errno == EINTR) {
            continue;
        }
        if (n <= 0) {
            return -1;
        }
        done += (size_t)n;
    }
    return 0;
}

static int read_all(int fd, void *buf, size_t len) {
    unsigned char *bytes = buf;
    size_t done = 0;
    while (done < len) {
        ssize_t n = read(fd, bytes + done, len - done);
        if (n < 0 && errno == EINTR) {
            continue;
        }
        if (n <= 0) {
            return -1;
        }
        done += (size_t)n;
    }
    return 0;
}

int main(void) {
    char src_path[] = "/tmp/kx-sendfile-src-XXXXXX";
    char dst_path[] = "/tmp/kx-sendfile-dst-XXXXXX";
    unsigned char payload[DATA_SIZE];
    unsigned char buf[DATA_SIZE];
    unsigned char filler[PIPE_CHUNK_SIZE];
    int pipefd[2] = {-1, -1};
    int src = -1;
    int dst = -1;
    int result = 1;
    size_t filled = 0;
    off_t offset;
    ssize_t n;

    alarm(30);
    for (size_t i = 0; i < sizeof(payload); i++) {
        payload[i] = (unsigned char)(i * 37 + 11);
    }
    memset(filler, 'Q', sizeof(filler));

    src = mkstemp(src_path);
    dst = mkstemp(dst_path);
    if (src < 0 || dst < 0 || write_all(src, payload, sizeof(payload)) < 0 || lseek(src, 0, SEEK_SET) != 0) {
        perror("prepare sendfile files");
        goto out;
    }

    offset = 113;
    n = sendfile(dst, src, &offset, 1500);
    if (n != 1500 || offset != 1613 || lseek(src, 0, SEEK_CUR) != 0 || lseek(dst, 0, SEEK_CUR) != 1500) {
        fprintf(stderr, "sendfile explicit offset: n=%zd offset=%lld\n", n, (long long)offset);
        goto out;
    }
    if (lseek(dst, 0, SEEK_SET) != 0 || read_all(dst, buf, 1500) < 0 || memcmp(buf, payload + 113, 1500) != 0) {
        fprintf(stderr, "sendfile explicit offset: content mismatch\n");
        goto out;
    }

    if (ftruncate(dst, 0) < 0 || lseek(dst, 0, SEEK_SET) != 0) {
        perror("reset sendfile destination");
        goto out;
    }
    struct rlimit old_limit;
    if (getrlimit(RLIMIT_FSIZE, &old_limit) < 0) {
        perror("getrlimit RLIMIT_FSIZE");
        goto out;
    }
    if (old_limit.rlim_cur < SHORT_WRITE_SIZE) {
        fprintf(stderr, "RLIMIT_FSIZE is already below %d\n", SHORT_WRITE_SIZE);
        goto out;
    }
    struct rlimit limited = old_limit;
    /* Limit the second 1 KiB write to 512 bytes. */
    limited.rlim_cur = SHORT_WRITE_SIZE;
    if (signal(SIGXFSZ, SIG_IGN) == SIG_ERR || setrlimit(RLIMIT_FSIZE, &limited) < 0) {
        perror("setrlimit RLIMIT_FSIZE");
        goto out;
    }
    n = sendfile(dst, src, NULL, 2 * CHUNK_SIZE);
    if (setrlimit(RLIMIT_FSIZE, &old_limit) < 0) {
        perror("restore RLIMIT_FSIZE");
        goto out;
    }
    if (n != SHORT_WRITE_SIZE || lseek(src, 0, SEEK_CUR) != SHORT_WRITE_SIZE) {
        fprintf(stderr, "sendfile short write: n=%zd source offset=%lld\n", n, (long long)lseek(src, 0, SEEK_CUR));
        goto out;
    }
    n = sendfile(dst, src, NULL, 2 * CHUNK_SIZE - SHORT_WRITE_SIZE);
    if (n != 2 * CHUNK_SIZE - SHORT_WRITE_SIZE || lseek(src, 0, SEEK_CUR) != 2 * CHUNK_SIZE) {
        fprintf(stderr, "sendfile after short write: n=%zd\n", n);
        goto out;
    }
    if (lseek(dst, 0, SEEK_SET) != 0 || read_all(dst, buf, 2 * CHUNK_SIZE) < 0 ||
        memcmp(buf, payload, 2 * CHUNK_SIZE) != 0) {
        fprintf(stderr, "sendfile after short write: content mismatch\n");
        goto out;
    }

    if (fcntl(dst, F_SETFL, O_APPEND) < 0) {
        perror("set O_APPEND");
        goto out;
    }
    errno = 0;
    n = sendfile(dst, src, NULL, 1);
    if (n != -1 || errno != EINVAL || lseek(src, 0, SEEK_CUR) != 2 * CHUNK_SIZE) {
        fprintf(stderr, "sendfile O_APPEND: n=%zd errno=%d\n", n, errno);
        goto out;
    }
    if (fcntl(dst, F_SETFL, 0) < 0) {
        perror("clear O_APPEND");
        goto out;
    }
    offset = -1;
    errno = 0;
    n = sendfile(dst, src, &offset, 1);
    if (n != -1 || errno != EINVAL) {
        fprintf(stderr, "sendfile negative offset: n=%zd errno=%d\n", n, errno);
        goto out;
    }
    offset = LLONG_MAX;
    errno = 0;
    n = sendfile(dst, src, &offset, 1);
    if (n != -1 || errno != EOVERFLOW) {
        fprintf(stderr, "sendfile offset overflow: n=%zd errno=%d\n", n, errno);
        goto out;
    }
    offset = 0;
    errno = 0;
    n = sendfile(dst, src, &offset, (size_t)-1);
    if (n != -1 || errno != EINVAL) {
        fprintf(stderr, "sendfile count overflow: n=%zd errno=%d\n", n, errno);
        goto out;
    }

    if (pipe(pipefd) < 0 || fcntl(pipefd[1], F_SETFL, O_NONBLOCK) < 0) {
        perror("prepare sendfile pipe");
        goto out;
    }
    offset = 0;
    errno = 0;
    n = sendfile(dst, pipefd[0], &offset, 1);
    if (n != -1 || errno != ESPIPE) {
        fprintf(stderr, "sendfile pipe input offset: n=%zd errno=%d\n", n, errno);
        goto out;
    }
    if (lseek(src, 0, SEEK_SET) != 0) {
        perror("reset sendfile source");
        goto out;
    }

    for (;;) {
        n = write(pipefd[1], filler, sizeof(filler));
        if (n == (ssize_t)sizeof(filler)) {
            filled += (size_t)n;
            continue;
        }
        if (n < 0 && errno == EAGAIN) {
            break;
        }
        fprintf(stderr, "fill sendfile pipe: n=%zd errno=%d\n", n, errno);
        goto out;
    }
    /* Free one page so the transfer makes progress before the pipe fills again. */
    if (filled < sizeof(filler) || read_all(pipefd[0], buf, sizeof(filler)) < 0) {
        fprintf(stderr, "drain sendfile pipe failed\n");
        goto out;
    }

    n = sendfile(pipefd[1], src, NULL, sizeof(payload));
    if (n <= 0 || n >= (ssize_t)sizeof(payload) || lseek(src, 0, SEEK_CUR) != n) {
        fprintf(stderr, "sendfile partial pipe write: n=%zd errno=%d\n", n, errno);
        goto out;
    }
    size_t sent = (size_t)n;
    size_t filler_left = filled - sizeof(filler);
    while (filler_left > 0) {
        size_t chunk = filler_left < sizeof(buf) ? filler_left : sizeof(buf);
        if (read_all(pipefd[0], buf, chunk) < 0) {
            fprintf(stderr, "read sendfile pipe filler failed\n");
            goto out;
        }
        for (size_t i = 0; i < chunk; i++) {
            if (buf[i] != 'Q') {
                fprintf(stderr, "sendfile pipe filler changed\n");
                goto out;
            }
        }
        filler_left -= chunk;
    }
    if (read_all(pipefd[0], buf, sent) < 0 || memcmp(buf, payload, sent) != 0) {
        fprintf(stderr, "sendfile partial pipe write: content mismatch\n");
        goto out;
    }

    while (sent < sizeof(payload)) {
        n = sendfile(pipefd[1], src, NULL, sizeof(payload) - sent);
        if (n <= 0 || lseek(src, 0, SEEK_CUR) != (off_t)(sent + n)) {
            fprintf(stderr, "sendfile pipe retry: n=%zd errno=%d\n", n, errno);
            goto out;
        }
        if (read_all(pipefd[0], buf, (size_t)n) < 0 || memcmp(buf, payload + sent, (size_t)n) != 0) {
            fprintf(stderr, "sendfile pipe retry: content mismatch\n");
            goto out;
        }
        sent += (size_t)n;
    }

    puts("sendfile semantics ok");
    result = 0;

out:
    if (pipefd[0] >= 0) close(pipefd[0]);
    if (pipefd[1] >= 0) close(pipefd[1]);
    if (src >= 0) close(src);
    if (dst >= 0) close(dst);
    unlink(src_path);
    unlink(dst_path);
    return result;
}
