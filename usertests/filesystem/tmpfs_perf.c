#include <errno.h>
#include <fcntl.h>
#include <stdio.h>
#include <sys/mount.h>
#include <sys/syscall.h>
#include <sys/stat.h>
#include <sys/time.h>
#include <unistd.h>

enum {
    BUFFER_SIZE = 1024,
    FILE_SIZE = 1024 * 1024,
};

static const char *TEST_FILE = "/tmp/tmpfs_perf_file";
static const char *TMP_DIR = "/tmp";

static unsigned long long now_us(void) {
    struct timeval tv;

    syscall(SYS_gettimeofday, &tv, NULL);
    return (unsigned long long)tv.tv_sec * 1000000 + tv.tv_usec;
}

int main(void) {
    char buffer[BUFFER_SIZE];

    if (mkdir(TMP_DIR, 0755) < 0 && errno != EEXIST) {
        perror("mkdir /tmp");
        return 1;
    }
    if (mount("tmpfs", TMP_DIR, "tmpfs", 0, NULL) < 0 && errno != EBUSY) {
        perror("mount tmpfs");
        return 1;
    }

    int fd = open(TEST_FILE, O_CREAT | O_TRUNC | O_RDWR, 0644);
    if (fd < 0) {
        perror("open");
        return 1;
    }

    for (int i = 0; i < BUFFER_SIZE; i++) {
        buffer[i] = (char)(i & 0xff);
    }

    unsigned long long start = now_us();
    for (int written = 0; written < FILE_SIZE;) {
        int remaining = FILE_SIZE - written;
        size_t chunk = remaining < BUFFER_SIZE ? (size_t)remaining : sizeof(buffer);
        ssize_t ret = write(fd, buffer, chunk);
        if (ret <= 0) {
            perror("write");
            close(fd);
            unlink(TEST_FILE);
            return 1;
        }
        written += ret;
    }
    unsigned long long elapsed = now_us() - start;
    if (elapsed == 0) {
        elapsed = 1;
    }
    printf("tmpfs write: %d KB in %llu us (%llu KB/s)\n",
           FILE_SIZE / 1024,
           elapsed,
           (unsigned long long)(FILE_SIZE / 1024) * 1000000 / elapsed);

    if (lseek(fd, 0, SEEK_SET) < 0) {
        perror("lseek");
        close(fd);
        unlink(TEST_FILE);
        return 1;
    }

    start = now_us();
    for (int read_bytes = 0; read_bytes < FILE_SIZE;) {
        ssize_t ret = read(fd, buffer, sizeof(buffer));
        if (ret <= 0) {
            perror("read");
            close(fd);
            unlink(TEST_FILE);
            return 1;
        }
        read_bytes += ret;
    }
    elapsed = now_us() - start;
    if (elapsed == 0) {
        elapsed = 1;
    }
    printf("tmpfs read: %d KB in %llu us (%llu KB/s)\n",
           FILE_SIZE / 1024,
           elapsed,
           (unsigned long long)(FILE_SIZE / 1024) * 1000000 / elapsed);

    close(fd);
    unlink(TEST_FILE);
    return 0;
}
