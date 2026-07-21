#define _GNU_SOURCE

/* Preserve MAP_SHARED PTE dirty state across _exit and exec teardown. */

#include "swap_test_common.h"

#include <fcntl.h>
#include <signal.h>
#include <sys/wait.h>

enum {
    PRESSURE_PASSES = 2,
    WATCHDOG_SECONDS = 300,
};

enum teardown_mode {
    TEARDOWN_EXIT,
    TEARDOWN_EXEC,
};

static int setup_file(const char *path, size_t target_size)
{
    int fd = open(path, O_RDWR | O_CREAT | O_TRUNC, 0600);

    if (fd < 0 || ftruncate(fd, (off_t)target_size) != 0 || fsync(fd) != 0) {
        fprintf(stderr, "pte_dirty_teardown_race: setup %s failed: %s\n", path, strerror(errno));
        if (fd >= 0) {
            close(fd);
        }
        return -1;
    }
    return close(fd);
}

static int write_mapping(const char *path, size_t target_size, size_t page_size, unsigned int generation)
{
    size_t page_count = target_size / page_size;
    uint8_t *mapping;
    int fd;
    int result;

    fd = open(path, O_RDWR);
    if (fd < 0) {
        fprintf(stderr, "pte_dirty_teardown_race: child open %s failed: %s\n", path, strerror(errno));
        return -1;
    }
    mapping = mmap(NULL, target_size, PROT_READ | PROT_WRITE, MAP_SHARED, fd, 0);
    if (mapping == MAP_FAILED) {
        fprintf(stderr, "pte_dirty_teardown_race: child mmap %s failed: %s\n", path, strerror(errno));
        close(fd);
        return -1;
    }

    for (size_t page = 0; page < page_count; page++) {
        swap_test_store_record(
            swap_test_record(mapping, page, page_size, 0), page, 0, generation);
        swap_test_store_record(
            swap_test_record(mapping, page, page_size, 1), page, 1, generation);
        atomic_store_explicit(
            swap_test_tail(mapping, page, page_size),
            swap_test_tail_canary(page),
            memory_order_release);
    }
    result = swap_test_verify_mapping(
        "pte_dirty_teardown_race", mapping, page_count, page_size, generation, 0);
    close(fd);

    /*
     * Deliberately leave mapping installed. The caller must terminate the old
     * address space through _exit or exec without munmap, msync, or fsync.
     */
    return result;
}

static int wait_child(pid_t child)
{
    int status;

    while (waitpid(child, &status, 0) < 0) {
        if (errno != EINTR) {
            return -1;
        }
    }
    return WIFEXITED(status) && WEXITSTATUS(status) == 0 ? 0 : -1;
}

static int run_teardown(
    const char *self,
    const char *path,
    enum teardown_mode mode,
    size_t target_size,
    size_t page_size,
    unsigned int generation)
{
    pid_t child;

    if (setup_file(path, target_size) != 0) {
        return -1;
    }

    fflush(NULL);
    child = fork();
    if (child < 0) {
        fprintf(stderr, "pte_dirty_teardown_race: fork failed: %s\n", strerror(errno));
        return -1;
    }
    if (child == 0) {
        alarm(WATCHDOG_SECONDS);
        if (write_mapping(path, target_size, page_size, generation) != 0) {
            _exit(1);
        }
        if (mode == TEARDOWN_EXEC) {
            execl(self, self, "--after-exec", NULL);
            _exit(1);
        }
        _exit(0);
    }

    if (wait_child(child) != 0) {
        fprintf(
            stderr,
            "pte_dirty_teardown_race: %s child failed\n",
            mode == TEARDOWN_EXEC ? "exec" : "exit");
        return -1;
    }
    return 0;
}

static int verify_file(
    const char *path,
    size_t target_size,
    size_t page_size,
    unsigned int generation)
{
    size_t page_count = target_size / page_size;
    uint8_t *buffer = malloc(page_size);
    int fd = open(path, O_RDONLY);
    int result = -1;

    if (buffer == NULL || fd < 0) {
        fprintf(stderr, "pte_dirty_teardown_race: verify setup failed: %s\n", strerror(errno));
        goto cleanup;
    }
    for (size_t page = 0; page < page_count; page++) {
        struct swap_record_values values;
        uint64_t tail;
        ssize_t length = pread(fd, buffer, page_size, (off_t)(page * page_size));

        if (length != (ssize_t)page_size) {
            fprintf(
                stderr,
                "pte_dirty_teardown_race: pread page=%zu failed: %s\n",
                page,
                length < 0 ? strerror(errno) : "short read");
            goto cleanup;
        }
        memcpy(&values, buffer, sizeof(values));
        if (swap_test_validate_values(
                "pte_dirty_teardown_race", &values, page, 0, generation) != 0) {
            goto cleanup;
        }
        memcpy(&values, buffer + page_size / 2, sizeof(values));
        if (swap_test_validate_values(
                "pte_dirty_teardown_race", &values, page, 1, generation) != 0) {
            goto cleanup;
        }
        memcpy(&tail, buffer + page_size - sizeof(tail), sizeof(tail));
        if (tail != swap_test_tail_canary(page)) {
            fprintf(stderr, "pte_dirty_teardown_race: tail mismatch page=%zu\n", page);
            goto cleanup;
        }
    }
    result = 0;

cleanup:
    if (fd >= 0) {
        close(fd);
    }
    free(buffer);
    return result;
}

static void watchdog_handler(int signal_number)
{
    (void)signal_number;
    _exit(124);
}

int main(int argc, char **argv)
{
    const char *paths[] = {
        "/pte_dirty_exit.data",
        "/pte_dirty_exec.data",
    };
    size_t target_size = SWAP_TEST_DEFAULT_TARGET_MIB * SWAP_TEST_MIB;
    size_t pressure_extra = SWAP_TEST_DEFAULT_PRESSURE_MIB * SWAP_TEST_MIB;
    size_t page_size;
    size_t pressure_size;
    volatile uint8_t *pressure = MAP_FAILED;
    int result = 1;

    if (argc == 2 && strcmp(argv[1], "--after-exec") == 0) {
        return 0;
    }
    if (argc > 3) {
        fprintf(stderr, "usage: %s [target_mib] [pressure_extra_mib]\n", argv[0]);
        return 2;
    }

    page_size = swap_test_page_size("pte_dirty_teardown_race");
    swap_test_require_lock_free_atomics("pte_dirty_teardown_race");
    if (argc >= 2) {
        target_size = swap_test_parse_mib(argv[1], "target size");
    }
    if (argc >= 3) {
        pressure_extra = swap_test_parse_mib(argv[2], "pressure extra size");
    }
    target_size = target_size / page_size * page_size;
    pressure_size = swap_test_pressure_size(page_size, pressure_extra);

    signal(SIGALRM, watchdog_handler);
    alarm(WATCHDOG_SECONDS);
    pressure = mmap(
        NULL,
        pressure_size,
        PROT_READ | PROT_WRITE,
        MAP_PRIVATE | MAP_ANONYMOUS,
        -1,
        0);
    if (pressure == MAP_FAILED) {
        fprintf(stderr, "pte_dirty_teardown_race: pressure mmap failed: %s\n", strerror(errno));
        goto cleanup;
    }

    printf(
        "pte_dirty_teardown_race: file=%zu MiB pressure=%zu MiB\n",
        target_size / SWAP_TEST_MIB,
        pressure_size / SWAP_TEST_MIB);
    for (unsigned int mode = TEARDOWN_EXIT; mode <= TEARDOWN_EXEC; mode++) {
        unsigned int generation = mode + 1;

        if (run_teardown(
                argv[0],
                paths[mode],
                (enum teardown_mode)mode,
                target_size,
                page_size,
                generation) != 0) {
            goto cleanup;
        }
        for (unsigned int pass = 1; pass <= PRESSURE_PASSES; pass++) {
            swap_test_touch_pressure(
                pressure,
                pressure_size / page_size,
                page_size,
                generation * 16 + pass,
                "pte_dirty_teardown_race");
        }
        if (verify_file(paths[mode], target_size, page_size, generation) != 0) {
            fprintf(
                stderr,
                "pte_dirty_teardown_race: %s teardown lost mmap writes\n",
                mode == TEARDOWN_EXEC ? "exec" : "exit");
            goto cleanup;
        }
        printf(
            "pte_dirty_teardown_race: %s teardown passed\n",
            mode == TEARDOWN_EXEC ? "exec" : "exit");
    }
    result = 0;

cleanup:
    alarm(0);
    if (pressure != MAP_FAILED) {
        munmap((void *)pressure, pressure_size);
    }
    for (size_t index = 0; index < sizeof(paths) / sizeof(paths[0]); index++) {
        unlink(paths[index]);
    }
    return result;
}
