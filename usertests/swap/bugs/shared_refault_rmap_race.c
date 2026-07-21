#define _GNU_SOURCE

/* Concurrent shared-anonymous/shared-file refault and reverse-map test. */

#include "swap_test_common.h"

#include <fcntl.h>
#include <pthread.h>
#include <signal.h>
#include <sys/wait.h>

enum {
    DEFAULT_ROUNDS = 8,
    MAX_ROUNDS = 64,
    WORKER_COUNT = 2,
    WATCHDOG_SECONDS = 300,
};

enum target_mode {
    TARGET_SHARED_ANONYMOUS,
    TARGET_SHARED_FILE,
};

struct pressure_context {
    volatile uint8_t *mapping;
    size_t page_count;
    size_t page_size;
    unsigned int generation;
    const char *mode_name;
    _Atomic size_t progress;
    _Atomic int stop;
};

static int read_byte(int fd, char *token)
{
    ssize_t length;

    do {
        length = read(fd, token, 1);
    } while (length < 0 && errno == EINTR);
    return length == 1 ? 0 : -1;
}

static int write_byte(int fd, char token)
{
    ssize_t length;

    do {
        length = write(fd, &token, 1);
    } while (length < 0 && errno == EINTR);
    return length == 1 ? 0 : -1;
}

static void *pressure_worker(void *argument)
{
    struct pressure_context *context = argument;
    unsigned int pass = 0;

    while (!atomic_load_explicit(&context->stop, memory_order_relaxed)) {
        for (size_t page = 0; page < context->page_count; page++) {
            if (atomic_load_explicit(&context->stop, memory_order_relaxed)) {
                return NULL;
            }
            context->mapping[page * context->page_size] =
                (uint8_t)(page ^ ((context->generation + pass) * 0x5bU));
            if ((page & 255U) == 0) {
                atomic_store_explicit(&context->progress, page + 1, memory_order_release);
            }
            if ((page + 1) % SWAP_TEST_PROGRESS_PAGES == 0 || page + 1 == context->page_count) {
                printf(
                    "shared_refault_rmap_race[%s]: pressure pass %u: %zu/%zu pages\n",
                    context->mode_name,
                    pass + 1,
                    page + 1,
                    context->page_count);
                fflush(stdout);
            }
        }
        pass++;
        atomic_store_explicit(&context->progress, context->page_count, memory_order_release);
        printf(
            "shared_refault_rmap_race[%s]: pressure pass %u complete\n",
            context->mode_name,
            pass);
        fflush(stdout);
    }
    return NULL;
}

static int verify_replacement(uint8_t *base, size_t page_count, size_t page_size, unsigned int generation)
{
    for (size_t page = 0; page < page_count; page++) {
        uint64_t expected = UINT64_C(0x510e527fade682d1) ^ ((uint64_t)generation << 48) ^ page;
        uint64_t first = *(volatile uint64_t *)(base + page * page_size);
        uint64_t middle = *(volatile uint64_t *)(base + page * page_size + page_size / 2);
        uint64_t tail = *(volatile uint64_t *)(base + (page + 1) * page_size - sizeof(uint64_t));

        if (first != expected || middle != ~expected || tail != (expected ^ UINT64_MAX / 3)) {
            fprintf(stderr, "shared_refault_rmap_race: replacement mismatch page=%zu\n", page);
            return -1;
        }
    }
    return 0;
}

static int churn_mapping(
    enum target_mode mode,
    uint8_t *mapping,
    size_t target_size,
    size_t page_size,
    int fd,
    unsigned int generation)
{
    size_t page_count = target_size / page_size;
    size_t churn_pages = page_count >= 32 ? 16 : 1;
    size_t first_page = page_count / 2 - churn_pages / 2;
    size_t churn_size = churn_pages * page_size;
    uint8_t *churn_base = mapping + first_page * page_size;

    if (mode == TARGET_SHARED_ANONYMOUS) {
        if (mprotect(churn_base, churn_size, PROT_READ) != 0) {
            fprintf(stderr, "shared_refault_rmap_race: mprotect read-only failed: %s\n", strerror(errno));
            return -1;
        }
        sched_yield();
        if (mprotect(churn_base, churn_size, PROT_READ | PROT_WRITE) != 0) {
            fprintf(stderr, "shared_refault_rmap_race: mprotect restore failed: %s\n", strerror(errno));
            return -1;
        }
        return 0;
    }

    if (munmap(churn_base, churn_size) != 0 ||
        mmap(
            churn_base,
            churn_size,
            PROT_READ | PROT_WRITE,
            MAP_PRIVATE | MAP_ANONYMOUS | MAP_FIXED,
            -1,
            0) != churn_base) {
        fprintf(stderr, "shared_refault_rmap_race: MAP_FIXED replacement failed: %s\n", strerror(errno));
        return -1;
    }
    for (size_t page = 0; page < churn_pages; page++) {
        uint64_t value = UINT64_C(0x510e527fade682d1) ^ ((uint64_t)generation << 48) ^ page;

        *(volatile uint64_t *)(churn_base + page * page_size) = value;
        *(volatile uint64_t *)(churn_base + page * page_size + page_size / 2) = ~value;
        *(volatile uint64_t *)(churn_base + (page + 1) * page_size - sizeof(uint64_t)) = value ^ UINT64_MAX / 3;
    }
    if (mprotect(churn_base, churn_size, PROT_READ) != 0) {
        fprintf(stderr, "shared_refault_rmap_race: replacement mprotect failed: %s\n", strerror(errno));
        return -1;
    }
    sched_yield();
    if (verify_replacement(churn_base, churn_pages, page_size, generation) != 0 ||
        munmap(churn_base, churn_size) != 0 ||
        mmap(
            churn_base,
            churn_size,
            PROT_READ | PROT_WRITE,
            MAP_SHARED | MAP_FIXED,
            fd,
            (off_t)(first_page * page_size)) != churn_base) {
        fprintf(stderr, "shared_refault_rmap_race: original mapping restore failed: %s\n", strerror(errno));
        return -1;
    }
    return 0;
}

static int worker_main(
    uint8_t *mapping,
    size_t page_count,
    size_t page_size,
    unsigned int worker,
    unsigned int generation,
    int start_fd,
    int report_fd)
{
    char token;

    if (swap_test_verify_mapping(
            "shared_refault_rmap_race", mapping, page_count, page_size, generation - 1, worker != 0) != 0 ||
        write_byte(report_fd, 'R') != 0 || read_byte(start_fd, &token) != 0) {
        return 1;
    }

    for (size_t step = 0; step < page_count; step++) {
        size_t page = worker == 0 ? step : page_count - step - 1;

        swap_test_store_record(
            swap_test_record(mapping, page, page_size, worker), page, worker, generation);
        if ((step + 1) % SWAP_TEST_PROGRESS_PAGES == 0 || step + 1 == page_count) {
            printf(
                "shared_refault_rmap_race: worker %u generation %u: %zu/%zu pages\n",
                worker,
                generation,
                step + 1,
                page_count);
            fflush(stdout);
        }
    }

    if (write_byte(report_fd, 'W') != 0 || read_byte(start_fd, &token) != 0) {
        return 1;
    }
    return swap_test_verify_mapping(
               "shared_refault_rmap_race", mapping, page_count, page_size, generation, worker == 0) == 0
               ? 0
               : 1;
}

static int run_round(
    enum target_mode mode,
    uint8_t *mapping,
    int fd,
    size_t target_size,
    volatile uint8_t *pressure,
    size_t pressure_size,
    size_t page_size,
    unsigned int generation)
{
    int start_pipes[WORKER_COUNT][2];
    int report_pipes[WORKER_COUNT][2];
    pid_t children[WORKER_COUNT] = {-1, -1};
    size_t page_count = target_size / page_size;
    struct pressure_context pressure_context;
    pthread_t pressure_thread;
    char token;
    int pressure_started = 0;
    int result = -1;

    for (unsigned int worker = 0; worker < WORKER_COUNT; worker++) {
        start_pipes[worker][0] = -1;
        start_pipes[worker][1] = -1;
        report_pipes[worker][0] = -1;
        report_pipes[worker][1] = -1;
    }
    for (unsigned int worker = 0; worker < WORKER_COUNT; worker++) {
        if (pipe(start_pipes[worker]) != 0 || pipe(report_pipes[worker]) != 0) {
            fprintf(stderr, "shared_refault_rmap_race: pipe failed: %s\n", strerror(errno));
            goto cleanup;
        }
    }

    fflush(NULL);
    for (unsigned int worker = 0; worker < WORKER_COUNT; worker++) {
        children[worker] = fork();
        if (children[worker] < 0) {
            fprintf(stderr, "shared_refault_rmap_race: fork failed: %s\n", strerror(errno));
            goto cleanup;
        }
        if (children[worker] == 0) {
            int worker_result;

            alarm(WATCHDOG_SECONDS);
            for (unsigned int pipe_owner = 0; pipe_owner < WORKER_COUNT; pipe_owner++) {
                close(start_pipes[pipe_owner][1]);
                close(report_pipes[pipe_owner][0]);
                if (pipe_owner != worker) {
                    close(start_pipes[pipe_owner][0]);
                    close(report_pipes[pipe_owner][1]);
                }
            }
            if (munmap((void *)pressure, pressure_size) != 0) {
                _exit(1);
            }
            worker_result = worker_main(
                mapping,
                page_count,
                page_size,
                worker,
                generation,
                start_pipes[worker][0],
                report_pipes[worker][1]);
            _exit(worker_result);
        }
    }

    for (unsigned int worker = 0; worker < WORKER_COUNT; worker++) {
        close(start_pipes[worker][0]);
        start_pipes[worker][0] = -1;
        close(report_pipes[worker][1]);
        report_pipes[worker][1] = -1;
        if (read_byte(report_pipes[worker][0], &token) != 0 || token != 'R') {
            fprintf(stderr, "shared_refault_rmap_race: worker %u readiness failed\n", worker);
            goto cleanup;
        }
    }

    pressure_context = (struct pressure_context){
        .mapping = pressure,
        .page_count = pressure_size / page_size,
        .page_size = page_size,
        .generation = generation,
        .mode_name = mode == TARGET_SHARED_ANONYMOUS ? "anonymous" : "file",
    };
    atomic_init(&pressure_context.progress, 0);
    atomic_init(&pressure_context.stop, 0);
    if (pthread_create(&pressure_thread, NULL, pressure_worker, &pressure_context) != 0) {
        fprintf(stderr, "shared_refault_rmap_race: pressure worker creation failed\n");
        goto cleanup;
    }
    pressure_started = 1;
    while (atomic_load_explicit(&pressure_context.progress, memory_order_acquire) <
           pressure_context.page_count / 2) {
        sched_yield();
    }
    if (churn_mapping(mode, mapping, target_size, page_size, fd, generation) != 0) {
        goto cleanup;
    }

    for (unsigned int worker = 0; worker < WORKER_COUNT; worker++) {
        if (write_byte(start_pipes[worker][1], 'S') != 0) {
            fprintf(stderr, "shared_refault_rmap_race: worker %u start failed\n", worker);
            goto cleanup;
        }
    }
    for (unsigned int worker = 0; worker < WORKER_COUNT; worker++) {
        if (read_byte(report_pipes[worker][0], &token) != 0 || token != 'W') {
            fprintf(stderr, "shared_refault_rmap_race: worker %u write phase failed\n", worker);
            goto cleanup;
        }
    }
    atomic_store_explicit(&pressure_context.stop, 1, memory_order_relaxed);
    pthread_join(pressure_thread, NULL);
    pressure_started = 0;
    for (unsigned int worker = 0; worker < WORKER_COUNT; worker++) {
        if (write_byte(start_pipes[worker][1], 'V') != 0) {
            fprintf(stderr, "shared_refault_rmap_race: worker %u verify release failed\n", worker);
            goto cleanup;
        }
    }

    for (unsigned int worker = 0; worker < WORKER_COUNT; worker++) {
        int status = 0;
        pid_t waited = waitpid(children[worker], &status, 0);

        if (waited < 0) {
            fprintf(stderr, "shared_refault_rmap_race: waitpid failed: %s\n", strerror(errno));
            goto cleanup;
        }
        children[worker] = -1;
        if (!WIFEXITED(status) || WEXITSTATUS(status) != 0) {
            fprintf(
                stderr,
                "shared_refault_rmap_race: worker %u failed status=0x%x\n",
                worker,
                status);
            goto cleanup;
        }
    }

    if (swap_test_verify_mapping(
            "shared_refault_rmap_race", mapping, page_count, page_size, generation, generation & 1U) != 0) {
        goto cleanup;
    }
    result = 0;

cleanup:
    if (pressure_started) {
        atomic_store_explicit(&pressure_context.stop, 1, memory_order_relaxed);
        pthread_join(pressure_thread, NULL);
    }
    for (unsigned int worker = 0; worker < WORKER_COUNT; worker++) {
        if (children[worker] > 0) {
            kill(children[worker], SIGKILL);
            waitpid(children[worker], NULL, 0);
        }
        if (start_pipes[worker][0] >= 0) {
            close(start_pipes[worker][0]);
        }
        if (start_pipes[worker][1] >= 0) {
            close(start_pipes[worker][1]);
        }
        if (report_pipes[worker][0] >= 0) {
            close(report_pipes[worker][0]);
        }
        if (report_pipes[worker][1] >= 0) {
            close(report_pipes[worker][1]);
        }
    }
    return result;
}

static int verify_file(
    int fd,
    size_t page_count,
    size_t page_size,
    unsigned int generation,
    uint8_t *buffer)
{
    for (size_t page = 0; page < page_count; page++) {
        ssize_t length = pread(fd, buffer, page_size, (off_t)(page * page_size));

        if (length != (ssize_t)page_size) {
            fprintf(
                stderr,
                "shared_refault_rmap_race: pread failed: %s\n",
                length < 0 ? strerror(errno) : "short read");
            return -1;
        }
        for (unsigned int writer = 0; writer < WORKER_COUNT; writer++) {
            struct swap_record_values values;
            size_t offset = writer == 0 ? 0 : page_size / 2;

            memcpy(&values, buffer + offset, sizeof(values));
            if (swap_test_validate_values(
                    "shared_refault_rmap_race", &values, page, writer, generation) != 0) {
                return -1;
            }
        }
        uint64_t tail;
        memcpy(&tail, buffer + page_size - sizeof(tail), sizeof(tail));
        if (tail != swap_test_tail_canary(page)) {
            fprintf(stderr, "shared_refault_rmap_race: file tail mismatch page=%zu\n", page);
            return -1;
        }
    }
    return 0;
}

static int run_mode(
    enum target_mode mode,
    size_t target_size,
    volatile uint8_t *pressure,
    size_t pressure_size,
    size_t page_size,
    unsigned int rounds)
{
    const char *path = "/shared_refault_rmap_race.data";
    uint8_t *mapping = MAP_FAILED;
    uint8_t *buffer = NULL;
    size_t page_count = target_size / page_size;
    int fd = -1;
    int result = -1;

    if (mode == TARGET_SHARED_FILE) {
        fd = open(path, O_RDWR | O_CREAT | O_TRUNC, 0600);
        if (fd < 0 || ftruncate(fd, (off_t)target_size) != 0) {
            fprintf(stderr, "shared_refault_rmap_race: file setup failed: %s\n", strerror(errno));
            goto cleanup;
        }
        mapping = mmap(NULL, target_size, PROT_READ | PROT_WRITE, MAP_SHARED, fd, 0);
    } else {
        mapping = mmap(NULL, target_size, PROT_READ | PROT_WRITE, MAP_SHARED | MAP_ANONYMOUS, -1, 0);
    }
    if (mapping == MAP_FAILED) {
        fprintf(stderr, "shared_refault_rmap_race: mmap failed: %s\n", strerror(errno));
        goto cleanup;
    }

    swap_test_initialize_mapping(mapping, page_count, page_size);
    if (fd >= 0 && fsync(fd) != 0) {
        fprintf(stderr, "shared_refault_rmap_race: initial fsync failed: %s\n", strerror(errno));
        goto cleanup;
    }

    for (unsigned int generation = 1; generation <= rounds; generation++) {
        if (run_round(mode, mapping, fd, target_size, pressure, pressure_size, page_size, generation) != 0) {
            goto cleanup;
        }
        if (fd >= 0 && fsync(fd) != 0) {
            fprintf(stderr, "shared_refault_rmap_race: fsync failed: %s\n", strerror(errno));
            goto cleanup;
        }
        printf(
            "shared_refault_rmap_race[%s]: round %u/%u complete\n",
            mode == TARGET_SHARED_ANONYMOUS ? "anonymous" : "file",
            generation,
            rounds);
        fflush(stdout);
    }

    if (mode == TARGET_SHARED_FILE) {
        buffer = malloc(page_size);
        if (buffer == NULL || munmap(mapping, target_size) != 0) {
            fprintf(stderr, "shared_refault_rmap_race: file verification setup failed: %s\n", strerror(errno));
            goto cleanup;
        }
        mapping = MAP_FAILED;
        swap_test_touch_pressure(
            pressure,
            pressure_size / page_size,
            page_size,
            rounds + 1,
            "shared_refault_rmap_race[file]");
        swap_test_touch_pressure(
            pressure,
            pressure_size / page_size,
            page_size,
            rounds + 2,
            "shared_refault_rmap_race[file]");
        if (verify_file(fd, page_count, page_size, rounds, buffer) != 0) {
            goto cleanup;
        }
    }

    result = 0;

cleanup:
    free(buffer);
    if (mapping != MAP_FAILED && munmap(mapping, target_size) != 0) {
        result = -1;
    }
    if (fd >= 0 && close(fd) != 0) {
        result = -1;
    }
    if (fd >= 0 && unlink(path) != 0) {
        result = -1;
    }
    return result;
}

static void watchdog_handler(int signal_number)
{
    (void)signal_number;
    _exit(124);
}

int main(int argc, char **argv)
{
    unsigned int rounds = DEFAULT_ROUNDS;
    size_t target_size = SWAP_TEST_DEFAULT_TARGET_MIB * SWAP_TEST_MIB;
    size_t pressure_extra = SWAP_TEST_DEFAULT_PRESSURE_MIB * SWAP_TEST_MIB;
    size_t page_size = swap_test_page_size("shared_refault_rmap_race");
    size_t pressure_size;
    volatile uint8_t *pressure;
    int result = 1;

    if (argc > 4) {
        fprintf(stderr, "usage: %s [rounds] [target_mib] [pressure_extra_mib]\n", argv[0]);
        return 2;
    }
    swap_test_require_lock_free_atomics("shared_refault_rmap_race");
    if (argc >= 2) {
        rounds = swap_test_parse_count(argv[1], "round count", MAX_ROUNDS);
    }
    if (argc >= 3) {
        target_size = swap_test_parse_mib(argv[2], "target size");
    }
    if (argc >= 4) {
        pressure_extra = swap_test_parse_mib(argv[3], "pressure extra size");
    }

    target_size = target_size / page_size * page_size;
    pressure_size = swap_test_pressure_size(page_size, pressure_extra);
    printf(
        "shared_refault_rmap_race: target=%zu MiB pressure=%zu MiB pages=%zu rounds=%u workers=%u\n",
        target_size / SWAP_TEST_MIB,
        pressure_size / SWAP_TEST_MIB,
        target_size / page_size,
        rounds,
        WORKER_COUNT);

    signal(SIGALRM, watchdog_handler);
    signal(SIGPIPE, SIG_IGN);
    alarm(WATCHDOG_SECONDS);
    pressure = mmap(NULL, pressure_size, PROT_READ | PROT_WRITE, MAP_PRIVATE | MAP_ANONYMOUS, -1, 0);
    if (pressure == MAP_FAILED) {
        fprintf(stderr, "shared_refault_rmap_race: pressure mmap failed: %s\n", strerror(errno));
        return 1;
    }

    if (run_mode(TARGET_SHARED_ANONYMOUS, target_size, pressure, pressure_size, page_size, rounds) != 0 ||
        run_mode(TARGET_SHARED_FILE, target_size, pressure, pressure_size, page_size, rounds) != 0) {
        goto cleanup;
    }
    result = 0;

cleanup:
    if (munmap((void *)pressure, pressure_size) != 0) {
        result = 1;
    }
    alarm(0);
    if (result == 0) {
        puts("shared_refault_rmap_race: PASS");
    }
    return result;
}
