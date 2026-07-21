#define _GNU_SOURCE

/* Race user-buffer pins with final anonymous mapping-ref release. */

#include "swap_test_common.h"

#include <fcntl.h>
#include <pthread.h>
#include <signal.h>

enum {
    DEFAULT_ROUNDS = 256,
    MAX_ROUNDS = 4096,
    PRESSURE_PASSES = 2,
    WATCHDOG_SECONDS = 300,
};

struct relink_context {
    int fd;
    uint8_t *address;
    size_t target_size;
    size_t page_count;
    size_t page_size;
    unsigned int rounds;
    volatile uint8_t *pressure;
    size_t pressure_page_count;
    _Atomic unsigned int remap_generation;
    _Atomic unsigned int io_round;
    _Atomic unsigned int pressure_passes;
    _Atomic int remapper_done;
    _Atomic int io_done;
    _Atomic int stop_pressure;
    _Atomic int failed;
};

static void initialize_generation(
    uint8_t *mapping,
    size_t page_count,
    size_t page_size,
    unsigned int generation)
{
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
}

static void *remap_worker(void *argument)
{
    struct relink_context *context = argument;

    for (unsigned int generation = 1; generation <= context->rounds; generation++) {
        uint8_t *mapping;

        if (munmap(context->address, context->target_size) != 0) {
            fprintf(stderr, "zero_mapping_ref_relink_race: munmap failed: %s\n", strerror(errno));
            atomic_store_explicit(&context->failed, 1, memory_order_relaxed);
            break;
        }
        sched_yield();
        mapping = mmap(
            context->address,
            context->target_size,
            PROT_READ | PROT_WRITE,
            MAP_PRIVATE | MAP_ANONYMOUS | MAP_FIXED,
            -1,
            0);
        if (mapping != context->address) {
            fprintf(stderr, "zero_mapping_ref_relink_race: MAP_FIXED failed: %s\n", strerror(errno));
            atomic_store_explicit(&context->failed, 1, memory_order_relaxed);
            break;
        }

        /*
         * Only touch sparse locations while racing. A full record pass is done
         * after the workers stop, when validation can be deterministic.
         */
        *(volatile uint64_t *)mapping =
            UINT64_C(0x6a09e667f3bcc909) ^ generation;
        *(volatile uint64_t *)(mapping + context->target_size / 2) =
            UINT64_C(0xbb67ae8584caa73b) ^ generation;
        *(volatile uint64_t *)(mapping + context->target_size - sizeof(uint64_t)) =
            UINT64_C(0x3c6ef372fe94f82b) ^ generation;
        atomic_store_explicit(&context->remap_generation, generation, memory_order_release);
        if ((generation & 31U) == 0 || generation == context->rounds) {
            printf(
                "zero_mapping_ref_relink_race: remap %u/%u\n",
                generation,
                context->rounds);
            fflush(stdout);
        }
    }

    atomic_store_explicit(&context->remapper_done, 1, memory_order_release);
    return NULL;
}

static void *io_worker(void *argument)
{
    struct relink_context *context = argument;

    for (unsigned int round = 1; round <= context->rounds; round++) {
        ssize_t length = pwrite(
            context->fd,
            context->address,
            context->target_size,
            0);

        /*
         * EFAULT and short writes are valid because the other thread
         * deliberately tears down the user buffer during this syscall.
         */
        if (length < 0 && errno != EFAULT && errno != EINTR) {
            fprintf(stderr, "zero_mapping_ref_relink_race: pwrite failed: %s\n", strerror(errno));
            atomic_store_explicit(&context->failed, 1, memory_order_relaxed);
            break;
        }
        atomic_store_explicit(&context->io_round, round, memory_order_release);
        sched_yield();
    }

    atomic_store_explicit(&context->io_done, 1, memory_order_release);
    return NULL;
}

static void *pressure_worker(void *argument)
{
    struct relink_context *context = argument;
    unsigned int generation = 1;

    while (!atomic_load_explicit(&context->stop_pressure, memory_order_relaxed)) {
        swap_test_touch_pressure(
            context->pressure,
            context->pressure_page_count,
            context->page_size,
            generation,
            "zero_mapping_ref_relink_race");
        atomic_fetch_add_explicit(&context->pressure_passes, 1, memory_order_release);
        generation++;
    }
    return NULL;
}

static void watchdog_handler(int signal_number)
{
    (void)signal_number;
    _exit(124);
}

int main(int argc, char **argv)
{
    const char *path = "/zero_mapping_ref_relink_race.data";
    unsigned int rounds = DEFAULT_ROUNDS;
    size_t target_size = 4 * SWAP_TEST_MIB;
    size_t pressure_extra = SWAP_TEST_DEFAULT_PRESSURE_MIB * SWAP_TEST_MIB;
    size_t page_size = swap_test_page_size("zero_mapping_ref_relink_race");
    size_t pressure_size;
    uint8_t *mapping = MAP_FAILED;
    volatile uint8_t *pressure = MAP_FAILED;
    pthread_t remap_thread;
    pthread_t io_thread;
    pthread_t pressure_thread;
    struct relink_context context;
    int remap_started = 0;
    int io_started = 0;
    int pressure_started = 0;
    int fd = -1;
    int result = 1;

    if (argc > 4) {
        fprintf(
            stderr,
            "usage: %s [rounds] [target_mib] [pressure_extra_mib]\n",
            argv[0]);
        return 2;
    }
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

    signal(SIGALRM, watchdog_handler);
    alarm(WATCHDOG_SECONDS);
    fd = open(path, O_RDWR | O_CREAT | O_TRUNC, 0600);
    if (fd < 0 || ftruncate(fd, (off_t)target_size) != 0) {
        fprintf(stderr, "zero_mapping_ref_relink_race: file setup failed: %s\n", strerror(errno));
        goto cleanup;
    }
    mapping = mmap(
        NULL,
        target_size,
        PROT_READ | PROT_WRITE,
        MAP_PRIVATE | MAP_ANONYMOUS,
        -1,
        0);
    pressure = mmap(
        NULL,
        pressure_size,
        PROT_READ | PROT_WRITE,
        MAP_PRIVATE | MAP_ANONYMOUS,
        -1,
        0);
    if (mapping == MAP_FAILED || pressure == MAP_FAILED) {
        fprintf(stderr, "zero_mapping_ref_relink_race: mmap failed: %s\n", strerror(errno));
        goto cleanup;
    }
    memset(&context, 0, sizeof(context));
    context.fd = fd;
    context.address = mapping;
    context.target_size = target_size;
    context.page_count = target_size / page_size;
    context.page_size = page_size;
    context.rounds = rounds;
    context.pressure = pressure;
    context.pressure_page_count = pressure_size / page_size;
    atomic_init(&context.remap_generation, 0);
    atomic_init(&context.io_round, 0);
    atomic_init(&context.pressure_passes, 0);
    atomic_init(&context.remapper_done, 0);
    atomic_init(&context.io_done, 0);
    atomic_init(&context.stop_pressure, 0);
    atomic_init(&context.failed, 0);

    printf(
        "zero_mapping_ref_relink_race: buffer=%zu MiB pressure=%zu MiB rounds=%u\n",
        target_size / SWAP_TEST_MIB,
        pressure_size / SWAP_TEST_MIB,
        rounds);
    if (pthread_create(&pressure_thread, NULL, pressure_worker, &context) != 0) {
        fprintf(stderr, "zero_mapping_ref_relink_race: pressure pthread_create failed\n");
        goto cleanup;
    }
    pressure_started = 1;
    if (pthread_create(&io_thread, NULL, io_worker, &context) != 0) {
        fprintf(stderr, "zero_mapping_ref_relink_race: I/O pthread_create failed\n");
        goto cleanup;
    }
    io_started = 1;
    if (pthread_create(&remap_thread, NULL, remap_worker, &context) != 0) {
        fprintf(stderr, "zero_mapping_ref_relink_race: remap pthread_create failed\n");
        goto cleanup;
    }
    remap_started = 1;

    pthread_join(remap_thread, NULL);
    remap_started = 0;
    pthread_join(io_thread, NULL);
    io_started = 0;
    while (atomic_load_explicit(&context.pressure_passes, memory_order_acquire) <
           PRESSURE_PASSES) {
        sched_yield();
    }

    /*
     * Install a known final object at the reused address, then keep reclaim
     * active for one more pass. A stale zero-ref page must not disturb it.
     */
    if (munmap(mapping, target_size) != 0 ||
        mmap(
            mapping,
            target_size,
            PROT_READ | PROT_WRITE,
            MAP_PRIVATE | MAP_ANONYMOUS | MAP_FIXED,
            -1,
            0) != mapping) {
        fprintf(stderr, "zero_mapping_ref_relink_race: final MAP_FIXED failed: %s\n", strerror(errno));
        goto cleanup;
    }
    initialize_generation(mapping, context.page_count, page_size, rounds + 1);
    while (atomic_load_explicit(&context.pressure_passes, memory_order_acquire) <
           PRESSURE_PASSES + 1) {
        sched_yield();
    }
    atomic_store_explicit(&context.stop_pressure, 1, memory_order_release);
    pthread_join(pressure_thread, NULL);
    pressure_started = 0;

    if (atomic_load_explicit(&context.failed, memory_order_relaxed) ||
        swap_test_verify_mapping(
            "zero_mapping_ref_relink_race",
            mapping,
            context.page_count,
            page_size,
            rounds + 1,
            0) != 0) {
        goto cleanup;
    }
    result = 0;

cleanup:
    if (remap_started) {
        pthread_join(remap_thread, NULL);
    }
    if (io_started) {
        pthread_join(io_thread, NULL);
    }
    if (pressure_started) {
        atomic_store_explicit(&context.stop_pressure, 1, memory_order_release);
        pthread_join(pressure_thread, NULL);
    }
    alarm(0);
    if (mapping != MAP_FAILED) {
        munmap(mapping, target_size);
    }
    if (pressure != MAP_FAILED) {
        munmap((void *)pressure, pressure_size);
    }
    if (fd >= 0) {
        close(fd);
    }
    unlink(path);
    return result;
}
