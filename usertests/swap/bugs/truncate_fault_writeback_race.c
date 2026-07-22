#define _GNU_SOURCE

/* Race truncate invalidation with page-cache fault, pin, and writeback. */

#include "swap_test_common.h"

#include <fcntl.h>
#include <pthread.h>
#include <signal.h>
#include <stddef.h>
#include <sys/stat.h>

enum {
    DEFAULT_ROUNDS = 2048,
    MAX_ROUNDS = 100000,
    PRESSURE_PASSES = 2,
    WATCHDOG_SECONDS = 300,
};

struct truncate_context {
    int fd;
    size_t small_size;
    size_t large_size;
    size_t record_offset;
    unsigned int rounds;
    uint8_t *fault_buffer;
    size_t page_size;
    _Atomic unsigned int start_round;
    _Atomic unsigned int truncate_done;
    _Atomic unsigned int writer_done;
    _Atomic unsigned int fault_done;
    _Atomic int failed;
};

_Static_assert(sizeof(struct swap_page_record) == sizeof(struct swap_record_values), "record layout mismatch");

static int pwrite_all(int fd, const void *buffer, size_t length, off_t offset)
{
    const uint8_t *bytes = buffer;

    while (length != 0) {
        ssize_t written = pwrite(fd, bytes, length, offset);

        if (written < 0 && errno == EINTR) {
            continue;
        }
        if (written <= 0) {
            return -1;
        }
        bytes += written;
        length -= (size_t)written;
        offset += written;
    }
    return 0;
}

static int pread_all(int fd, void *buffer, size_t length, off_t offset)
{
    uint8_t *bytes = buffer;

    while (length != 0) {
        ssize_t read_length = pread(fd, bytes, length, offset);

        if (read_length < 0 && errno == EINTR) {
            continue;
        }
        if (read_length <= 0) {
            return -1;
        }
        bytes += read_length;
        length -= (size_t)read_length;
        offset += read_length;
    }
    return 0;
}

static void wait_for_round(_Atomic unsigned int *round_counter, unsigned int round)
{
    while (atomic_load_explicit(round_counter, memory_order_acquire) < round) {
        sched_yield();
    }
}

static void *truncate_worker(void *argument)
{
    struct truncate_context *context = argument;

    for (unsigned int round = 1; round <= context->rounds; round++) {
        wait_for_round(&context->start_round, round);
        if (ftruncate(context->fd, (off_t)context->small_size) != 0) {
            fprintf(stderr, "truncate_fault_writeback_race: ftruncate failed: %s\n", strerror(errno));
            atomic_store_explicit(&context->failed, 1, memory_order_relaxed);
        }
        atomic_store_explicit(&context->truncate_done, round, memory_order_release);
    }
    return NULL;
}

static void *writer_worker(void *argument)
{
    struct truncate_context *context = argument;

    for (unsigned int round = 1; round <= context->rounds; round++) {
        struct swap_record_values values =
            swap_test_record_values(3, 0, round);

        wait_for_round(&context->start_round, round);
        if (pwrite_all(
                context->fd,
                &values,
                sizeof(values),
                (off_t)context->record_offset) != 0) {
            fprintf(stderr, "truncate_fault_writeback_race: pwrite failed: %s\n", strerror(errno));
            atomic_store_explicit(&context->failed, 1, memory_order_relaxed);
        }
        atomic_store_explicit(&context->writer_done, round, memory_order_release);
    }
    return NULL;
}

static void *fault_worker(void *argument)
{
    struct truncate_context *context = argument;

    for (unsigned int round = 1; round <= context->rounds; round++) {
        ssize_t length;

        wait_for_round(&context->start_round, round);
        do {
            length = pread(
                context->fd,
                context->fault_buffer,
                context->page_size,
                (off_t)(context->large_size - context->page_size));
        } while (length < 0 && errno == EINTR);
        if (length < 0) {
            fprintf(stderr, "truncate_fault_writeback_race: concurrent pread failed: %s\n", strerror(errno));
            atomic_store_explicit(&context->failed, 1, memory_order_relaxed);
        }
        atomic_store_explicit(&context->fault_done, round, memory_order_release);
    }
    return NULL;
}

static int verify_round(struct truncate_context *context, unsigned int round)
{
    struct swap_record_values values;
    struct stat stat_buffer;

    if (fstat(context->fd, &stat_buffer) != 0) {
        fprintf(stderr, "truncate_fault_writeback_race: fstat failed: %s\n", strerror(errno));
        return -1;
    }
    if ((size_t)stat_buffer.st_size < context->record_offset + sizeof(values)) {
        return 0;
    }
    if (pread_all(
            context->fd,
            &values,
            sizeof(values),
            (off_t)context->record_offset) != 0 ||
        swap_test_validate_values(
            "truncate_fault_writeback_race", &values, 3, 0, round) != 0) {
        fprintf(
            stderr,
            "truncate_fault_writeback_race: non-serializable large-file result in round %u\n",
            round);
        return -1;
    }
    return 1;
}

static int verify_tail_zero(int fd, size_t page_size)
{
    uint8_t *buffer = malloc(page_size);
    size_t small_size = page_size + page_size / 2;
    int result = -1;

    if (buffer == NULL) {
        return -1;
    }
    memset(buffer, 0xa5, page_size);
    if (ftruncate(fd, (off_t)(2 * page_size)) != 0 ||
        pwrite_all(fd, buffer, page_size, (off_t)page_size) != 0 ||
        ftruncate(fd, (off_t)small_size) != 0 ||
        ftruncate(fd, (off_t)(2 * page_size)) != 0 ||
        pread_all(fd, buffer, page_size, (off_t)page_size) != 0) {
        fprintf(stderr, "truncate_fault_writeback_race: tail setup failed: %s\n", strerror(errno));
        goto cleanup;
    }
    for (size_t offset = 0; offset < page_size; offset++) {
        uint8_t expected = offset < page_size / 2 ? 0xa5 : 0;

        if (buffer[offset] != expected) {
            fprintf(
                stderr,
                "truncate_fault_writeback_race: tail zero mismatch offset=%zu value=%02x\n",
                offset,
                buffer[offset]);
            goto cleanup;
        }
    }
    result = 0;

cleanup:
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
    const char *path = "/truncate_fault_writeback_race.data";
    unsigned int rounds = DEFAULT_ROUNDS;
    size_t pressure_extra = SWAP_TEST_DEFAULT_PRESSURE_MIB * SWAP_TEST_MIB;
    size_t page_size = swap_test_page_size("truncate_fault_writeback_race");
    size_t pressure_size;
    volatile uint8_t *pressure = MAP_FAILED;
    pthread_t truncate_thread;
    pthread_t writer_thread;
    pthread_t fault_thread;
    struct truncate_context context;
    unsigned int verified_rounds = 0;
    int truncate_started = 0;
    int writer_started = 0;
    int fault_started = 0;
    int fd = -1;
    int result = 1;

    if (argc > 3) {
        fprintf(stderr, "usage: %s [rounds] [pressure_extra_mib]\n", argv[0]);
        return 2;
    }
    if (argc >= 2) {
        rounds = swap_test_parse_count(argv[1], "round count", MAX_ROUNDS);
    }
    if (argc >= 3) {
        pressure_extra = swap_test_parse_mib(argv[2], "pressure extra size");
    }
    pressure_size = swap_test_pressure_size(page_size, pressure_extra);

    signal(SIGALRM, watchdog_handler);
    alarm(WATCHDOG_SECONDS);
    fd = open(path, O_RDWR | O_CREAT | O_TRUNC, 0600);
    pressure = mmap(
        NULL,
        pressure_size,
        PROT_READ | PROT_WRITE,
        MAP_PRIVATE | MAP_ANONYMOUS,
        -1,
        0);
    if (fd < 0 || pressure == MAP_FAILED) {
        fprintf(stderr, "truncate_fault_writeback_race: setup failed: %s\n", strerror(errno));
        goto cleanup;
    }

    memset(&context, 0, sizeof(context));
    context.fd = fd;
    context.small_size = page_size + page_size / 2;
    context.large_size = 4 * page_size;
    context.record_offset = 3 * page_size;
    context.rounds = rounds;
    context.fault_buffer = malloc(page_size);
    context.page_size = page_size;
    atomic_init(&context.start_round, 0);
    atomic_init(&context.truncate_done, 0);
    atomic_init(&context.writer_done, 0);
    atomic_init(&context.fault_done, 0);
    atomic_init(&context.failed, 0);
    if (context.fault_buffer == NULL || ftruncate(fd, (off_t)context.large_size) != 0) {
        fprintf(stderr, "truncate_fault_writeback_race: initialization failed: %s\n", strerror(errno));
        goto cleanup;
    }

    printf(
        "truncate_fault_writeback_race: rounds=%u pressure=%zu MiB\n",
        rounds,
        pressure_size / SWAP_TEST_MIB);
    if (pthread_create(&truncate_thread, NULL, truncate_worker, &context) != 0) {
        goto cleanup;
    }
    truncate_started = 1;
    if (pthread_create(&writer_thread, NULL, writer_worker, &context) != 0) {
        goto cleanup;
    }
    writer_started = 1;
    if (pthread_create(&fault_thread, NULL, fault_worker, &context) != 0) {
        goto cleanup;
    }
    fault_started = 1;

    for (unsigned int round = 1; round <= rounds; round++) {
        if (ftruncate(fd, (off_t)context.small_size) != 0 ||
            ftruncate(fd, (off_t)context.large_size) != 0) {
            fprintf(stderr, "truncate_fault_writeback_race: round reset failed: %s\n", strerror(errno));
            atomic_store_explicit(&context.failed, 1, memory_order_relaxed);
            break;
        }
        atomic_store_explicit(&context.start_round, round, memory_order_release);
        wait_for_round(&context.truncate_done, round);
        wait_for_round(&context.writer_done, round);
        wait_for_round(&context.fault_done, round);
        if (atomic_load_explicit(&context.failed, memory_order_relaxed)) {
            break;
        }

        int verification = verify_round(&context, round);

        if (verification < 0) {
            atomic_store_explicit(&context.failed, 1, memory_order_relaxed);
            break;
        }
        verified_rounds += verification > 0;
        if ((round & 127U) == 0 || round == rounds) {
            printf(
                "truncate_fault_writeback_race: round %u/%u, large outcomes=%u\n",
                round,
                rounds,
                verified_rounds);
            fflush(stdout);
        }
    }

    pthread_join(truncate_thread, NULL);
    truncate_started = 0;
    pthread_join(writer_thread, NULL);
    writer_started = 0;
    pthread_join(fault_thread, NULL);
    fault_started = 0;
    if (atomic_load_explicit(&context.failed, memory_order_relaxed)) {
        goto cleanup;
    }
    if (verified_rounds == 0) {
        fprintf(stderr, "truncate_fault_writeback_race: no large-file outcome observed\n");
        goto cleanup;
    }
    if (verify_tail_zero(fd, page_size) != 0) {
        goto cleanup;
    }

    struct swap_record_values final_values =
        swap_test_record_values(3, 0, rounds + 1);
    if (ftruncate(fd, (off_t)context.large_size) != 0 ||
        pwrite_all(
            fd,
            &final_values,
            sizeof(final_values),
            (off_t)context.record_offset) != 0 ||
        fsync(fd) != 0) {
        fprintf(stderr, "truncate_fault_writeback_race: final write failed: %s\n", strerror(errno));
        goto cleanup;
    }
    for (unsigned int pass = 1; pass <= PRESSURE_PASSES; pass++) {
        swap_test_touch_pressure(
            pressure,
            pressure_size / page_size,
            page_size,
            pass,
            "truncate_fault_writeback_race");
    }
    struct swap_record_values actual_values;
    if (pread_all(
            fd,
            &actual_values,
            sizeof(actual_values),
            (off_t)context.record_offset) != 0 ||
        swap_test_validate_values(
            "truncate_fault_writeback_race",
            &actual_values,
            3,
            0,
            rounds + 1) != 0) {
        goto cleanup;
    }
    result = 0;

cleanup:
    /*
     * On an early setup failure no worker can be waiting. Once started, main
     * always publishes every round unless the worker itself reported failure.
     */
    if (truncate_started || writer_started || fault_started) {
        atomic_store_explicit(&context.start_round, rounds, memory_order_release);
    }
    if (truncate_started) {
        pthread_join(truncate_thread, NULL);
    }
    if (writer_started) {
        pthread_join(writer_thread, NULL);
    }
    if (fault_started) {
        pthread_join(fault_thread, NULL);
    }
    alarm(0);
    free(context.fault_buffer);
    if (pressure != MAP_FAILED) {
        munmap((void *)pressure, pressure_size);
    }
    if (fd >= 0) {
        close(fd);
    }
    unlink(path);
    return result;
}
