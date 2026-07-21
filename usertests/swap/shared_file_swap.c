#define _GNU_SOURCE

/* Shared disk-file mapping reclaim, writeback, and fork visibility test. */

#include <errno.h>
#include <fcntl.h>
#include <inttypes.h>
#include <signal.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/mman.h>
#include <sys/wait.h>
#include <unistd.h>

enum {
    DEFAULT_FILE_MIB = 32,
    DEFAULT_PRESSURE_MIB = 128,
    FALLBACK_MEMORY_MIB = 1024,
    MARKER_COUNT = 3,
    PROGRESS_PAGES = 16384,
};

static size_t parse_file_mib(const char *arg)
{
    char *end = NULL;
    unsigned long long mib;

    errno = 0;
    mib = strtoull(arg, &end, 0);
    if (errno != 0 || end == arg || *end != '\0' || mib == 0 || mib > SIZE_MAX / (1024UL * 1024UL)) {
        fprintf(stderr, "invalid file size in MiB: %s\n", arg);
        exit(2);
    }

    return (size_t)mib * 1024UL * 1024UL;
}

static size_t read_mem_total(void)
{
    FILE *file = fopen("/proc/meminfo", "r");
    char line[128];

    if (file == NULL) {
        return 0;
    }

    while (fgets(line, sizeof(line), file) != NULL) {
        unsigned long long kib;

        if (sscanf(line, "MemTotal: %llu kB", &kib) != 1) {
            continue;
        }
        fclose(file);
        if (kib > SIZE_MAX / 1024UL) {
            return 0;
        }
        return (size_t)kib * 1024UL;
    }

    fclose(file);
    return 0;
}

static size_t marker_offset(size_t page_size, unsigned int marker)
{
    switch (marker) {
    case 0:
        return 0;
    case 1:
        return (page_size / 2) & ~(sizeof(uint64_t) - 1);
    default:
        return page_size - sizeof(uint64_t);
    }
}

static uint64_t marker_value(size_t page, unsigned int marker, unsigned int generation)
{
    return UINT64_C(0xa54ff53a5f1d36f1) ^ ((uint64_t)page * UINT64_C(0x9e3779b97f4a7c15)) ^
           ((uint64_t)marker << 48) ^ ((uint64_t)generation << 56);
}

static void write_mapping(uint8_t *base, size_t page_count, size_t page_size, unsigned int generation)
{
    for (size_t page = 0; page < page_count; page++) {
        uint8_t *page_base = base + page * page_size;

        for (unsigned int marker = 0; marker < MARKER_COUNT; marker++) {
            size_t offset = marker_offset(page_size, marker);
            volatile uint64_t *value = (volatile uint64_t *)(page_base + offset);
            *value = marker_value(page, marker, generation);
        }

        if ((page + 1) % PROGRESS_PAGES == 0 || page + 1 == page_count) {
            printf("  wrote file generation %u: %zu/%zu pages\n", generation, page + 1, page_count);
            fflush(stdout);
        }
    }
}

static int verify_mapping(
    uint8_t *base,
    size_t page_count,
    size_t page_size,
    unsigned int generation,
    int reverse)
{
    for (size_t step = 0; step < page_count; step++) {
        size_t page = reverse ? page_count - step - 1 : step;
        uint8_t *page_base = base + page * page_size;

        for (unsigned int marker = 0; marker < MARKER_COUNT; marker++) {
            size_t offset = marker_offset(page_size, marker);
            volatile uint64_t *value = (volatile uint64_t *)(page_base + offset);
            uint64_t expected = marker_value(page, marker, generation);
            uint64_t actual = *value;

            if (actual != expected) {
                fprintf(
                    stderr,
                    "shared_file_swap: mapping mismatch page=%zu marker=%u generation=%u expected=%016" PRIx64
                    " actual=%016" PRIx64 "\n",
                    page,
                    marker,
                    generation,
                    expected,
                    actual);
                return -1;
            }
        }

        if ((step + 1) % PROGRESS_PAGES == 0 || step + 1 == page_count) {
            printf(
                "  verified file generation %u (%s): %zu/%zu pages\n",
                generation,
                reverse ? "reverse" : "forward",
                step + 1,
                page_count);
            fflush(stdout);
        }
    }

    return 0;
}

static void touch_pressure(volatile uint8_t *base, size_t page_count, size_t page_size, unsigned int generation)
{
    for (size_t page = 0; page < page_count; page++) {
        base[page * page_size] = (uint8_t)(page ^ (generation * 0x5bU));

        if ((page + 1) % PROGRESS_PAGES == 0 || page + 1 == page_count) {
            printf("  touched pressure generation %u: %zu/%zu pages\n", generation, page + 1, page_count);
            fflush(stdout);
        }
    }
}

static int verify_file(int fd, size_t page_count, size_t page_size, unsigned int generation)
{
    uint8_t *buffer = malloc(page_size);

    if (buffer == NULL) {
        fprintf(stderr, "shared_file_swap: verification buffer allocation failed\n");
        return -1;
    }

    for (size_t page = 0; page < page_count; page++) {
        off_t offset = (off_t)(page * page_size);
        ssize_t length = pread(fd, buffer, page_size, offset);

        if (length != (ssize_t)page_size) {
            fprintf(stderr, "shared_file_swap: pread failed: %s\n", length < 0 ? strerror(errno) : "short read");
            free(buffer);
            return -1;
        }

        for (unsigned int marker = 0; marker < MARKER_COUNT; marker++) {
            uint64_t actual;
            uint64_t expected = marker_value(page, marker, generation);

            memcpy(&actual, buffer + marker_offset(page_size, marker), sizeof(actual));
            if (actual != expected) {
                fprintf(
                    stderr,
                    "shared_file_swap: file mismatch page=%zu marker=%u generation=%u expected=%016" PRIx64
                    " actual=%016" PRIx64 "\n",
                    page,
                    marker,
                    generation,
                    expected,
                    actual);
                free(buffer);
                return -1;
            }
        }

        if ((page + 1) % PROGRESS_PAGES == 0 || page + 1 == page_count) {
            printf("  verified disk file generation %u: %zu/%zu pages\n", generation, page + 1, page_count);
            fflush(stdout);
        }
    }

    free(buffer);
    return 0;
}

int main(int argc, char **argv)
{
    const char *path = "/shared_file_swap.data";
    long page_size_value = sysconf(_SC_PAGESIZE);
    size_t memory_size = read_mem_total();
    size_t file_size = DEFAULT_FILE_MIB * 1024UL * 1024UL;
    size_t pressure_extra = DEFAULT_PRESSURE_MIB * 1024UL * 1024UL;
    size_t pressure_size;
    size_t page_size;
    size_t file_page_count;
    size_t pressure_page_count;
    uint8_t *mapping = MAP_FAILED;
    volatile uint8_t *pressure = MAP_FAILED;
    int ready_pipe[2] = {-1, -1};
    int continue_pipe[2] = {-1, -1};
    int fd = -1;
    pid_t pid = -1;
    int status;
    int result = 1;
    char token;

    if (page_size_value <= 0) {
        perror("sysconf(_SC_PAGESIZE)");
        return 1;
    }
    page_size = (size_t)page_size_value;
    if (page_size < sizeof(uint64_t) || page_size % sizeof(uint64_t) != 0) {
        fprintf(stderr, "shared_file_swap: unsupported page size %zu\n", page_size);
        return 1;
    }

    if (argc > 2) {
        fprintf(stderr, "usage: %s [file_mib]\n", argv[0]);
        return 2;
    }
    if (argc == 2) {
        file_size = parse_file_mib(argv[1]);
    }
    if (memory_size == 0) {
        memory_size = FALLBACK_MEMORY_MIB * 1024UL * 1024UL;
    }
    if (memory_size > SIZE_MAX - pressure_extra) {
        fprintf(stderr, "shared_file_swap: pressure size overflow\n");
        return 1;
    }

    file_size -= file_size % page_size;
    pressure_size = memory_size + pressure_extra;
    pressure_size -= pressure_size % page_size;
    file_page_count = file_size / page_size;
    pressure_page_count = pressure_size / page_size;
    printf(
        "shared_file_swap: MemTotal=%zu MiB file=%zu MiB pressure=%zu MiB file_pages=%zu page_size=%zu\n",
        memory_size / (1024UL * 1024UL),
        file_size / (1024UL * 1024UL),
        pressure_size / (1024UL * 1024UL),
        file_page_count,
        page_size);

    fd = open(path, O_RDWR | O_CREAT | O_TRUNC, 0600);
    if (fd < 0) {
        fprintf(stderr, "shared_file_swap: open failed: %s\n", strerror(errno));
        goto cleanup;
    }
    if (ftruncate(fd, (off_t)file_size) != 0) {
        fprintf(stderr, "shared_file_swap: ftruncate failed: %s\n", strerror(errno));
        goto cleanup;
    }

    mapping = mmap(NULL, file_size, PROT_READ | PROT_WRITE, MAP_SHARED, fd, 0);
    if (mapping == MAP_FAILED) {
        fprintf(stderr, "shared_file_swap: file mmap failed: %s\n", strerror(errno));
        goto cleanup;
    }
    write_mapping(mapping, file_page_count, page_size, 1);

    if (pipe(ready_pipe) != 0 || pipe(continue_pipe) != 0) {
        fprintf(stderr, "shared_file_swap: pipe failed: %s\n", strerror(errno));
        goto cleanup;
    }

    fflush(NULL);
    pid = fork();
    if (pid < 0) {
        fprintf(stderr, "shared_file_swap: fork failed: %s\n", strerror(errno));
        goto cleanup;
    }
    if (pid == 0) {
        close(ready_pipe[0]);
        close(continue_pipe[1]);

        if (verify_mapping(mapping, file_page_count, page_size, 1, 0) != 0 ||
            write(ready_pipe[1], "R", 1) != 1 || read(continue_pipe[0], &token, 1) != 1 ||
            verify_mapping(mapping, file_page_count, page_size, 1, 1) != 0) {
            _exit(1);
        }
        write_mapping(mapping, file_page_count, page_size, 2);
        _exit(0);
    }

    close(ready_pipe[1]);
    ready_pipe[1] = -1;
    close(continue_pipe[0]);
    continue_pipe[0] = -1;

    if (read(ready_pipe[0], &token, 1) != 1) {
        fprintf(stderr, "shared_file_swap: child readiness wait failed: %s\n", strerror(errno));
        kill(pid, SIGKILL);
        waitpid(pid, NULL, 0);
        pid = -1;
        goto cleanup;
    }
    close(ready_pipe[0]);
    ready_pipe[0] = -1;

    pressure = mmap(NULL, pressure_size, PROT_READ | PROT_WRITE, MAP_PRIVATE | MAP_ANONYMOUS, -1, 0);
    if (pressure == MAP_FAILED) {
        fprintf(stderr, "shared_file_swap: pressure mmap failed: %s\n", strerror(errno));
        kill(pid, SIGKILL);
        waitpid(pid, NULL, 0);
        pid = -1;
        goto cleanup;
    }
    touch_pressure(pressure, pressure_page_count, page_size, 1);

    if (write(continue_pipe[1], "C", 1) != 1) {
        fprintf(stderr, "shared_file_swap: child continue signal failed: %s\n", strerror(errno));
        kill(pid, SIGKILL);
        waitpid(pid, NULL, 0);
        pid = -1;
        goto cleanup;
    }
    close(continue_pipe[1]);
    continue_pipe[1] = -1;

    if (waitpid(pid, &status, 0) < 0) {
        fprintf(stderr, "shared_file_swap: waitpid failed: %s\n", strerror(errno));
        goto cleanup;
    }
    pid = -1;
    if (!WIFEXITED(status) || WEXITSTATUS(status) != 0) {
        fprintf(stderr, "shared_file_swap: child failed status=0x%x\n", status);
        goto cleanup;
    }

    touch_pressure(pressure, pressure_page_count, page_size, 2);
    if (verify_mapping(mapping, file_page_count, page_size, 2, 1) != 0) {
        goto cleanup;
    }

    if (munmap(mapping, file_size) != 0) {
        fprintf(stderr, "shared_file_swap: file munmap failed: %s\n", strerror(errno));
        goto cleanup;
    }
    mapping = MAP_FAILED;
    touch_pressure(pressure, pressure_page_count, page_size, 3);
    if (verify_file(fd, file_page_count, page_size, 2) != 0) {
        goto cleanup;
    }

    result = 0;

cleanup:
    if (pid > 0) {
        kill(pid, SIGKILL);
        waitpid(pid, NULL, 0);
    }
    if (ready_pipe[0] >= 0) {
        close(ready_pipe[0]);
    }
    if (ready_pipe[1] >= 0) {
        close(ready_pipe[1]);
    }
    if (continue_pipe[0] >= 0) {
        close(continue_pipe[0]);
    }
    if (continue_pipe[1] >= 0) {
        close(continue_pipe[1]);
    }
    if (pressure != MAP_FAILED && munmap((void *)pressure, pressure_size) != 0) {
        fprintf(stderr, "shared_file_swap: pressure munmap failed: %s\n", strerror(errno));
        result = 1;
    }
    if (mapping != MAP_FAILED && munmap(mapping, file_size) != 0) {
        fprintf(stderr, "shared_file_swap: file munmap failed: %s\n", strerror(errno));
        result = 1;
    }
    if (fd >= 0 && close(fd) != 0) {
        fprintf(stderr, "shared_file_swap: close failed: %s\n", strerror(errno));
        result = 1;
    }
    if (fd >= 0 && unlink(path) != 0) {
        fprintf(stderr, "shared_file_swap: unlink failed: %s\n", strerror(errno));
        result = 1;
    }

    if (result == 0) {
        puts("shared_file_swap: PASS");
    }
    return result;
}
