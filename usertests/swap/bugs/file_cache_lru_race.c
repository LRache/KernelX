#define _GNU_SOURCE

/* File-page LRU relink, fsync, and reclaim concurrency test. */

#include "swap_test_common.h"

#include <fcntl.h>
#include <stddef.h>
#include <pthread.h>
#include <signal.h>
#include <sys/stat.h>

enum {
    DEFAULT_ROUNDS = 12,
    MAX_ROUNDS = 64,
    WRITER_COUNT = 2,
    WATCHDOG_SECONDS = 300,
};

struct file_race_context {
    int fd;
    uint8_t *read_alias;
    uint8_t *write_alias;
    volatile uint8_t *pressure;
    size_t page_count;
    size_t page_size;
    size_t pressure_page_count;
    unsigned int rounds;
    unsigned int *last_generation;
    _Atomic int writers_done;
    _Atomic int stop_pressure;
    _Atomic int failed;
};

struct writer_context {
    struct file_race_context *race;
    unsigned int writer;
};

_Static_assert(sizeof(struct swap_page_record) == sizeof(struct swap_record_values), "record layout mismatch");

static off_t record_offset(size_t page, size_t page_size, unsigned int writer)
{
    return (off_t)(page * page_size + (writer == 0 ? 0 : page_size / 2));
}

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

static int pwrite_record(
    int fd,
    size_t page,
    size_t page_size,
    unsigned int writer,
    unsigned int generation)
{
    struct swap_record_values values = swap_test_record_values(page, writer, generation);
    uint64_t invalid_commit = 0;
    off_t offset = record_offset(page, page_size, writer);
    off_t commit_offset = offset + offsetof(struct swap_record_values, commit);

    if (pwrite_all(fd, &invalid_commit, sizeof(invalid_commit), commit_offset) != 0 ||
        pwrite_all(fd, &values, offsetof(struct swap_record_values, commit), offset) != 0 ||
        pwrite_all(fd, &values.commit, sizeof(values.commit), commit_offset) != 0) {
        return -1;
    }
    return 0;
}

static int validate_live_record(
    struct file_race_context *context,
    size_t page,
    unsigned int writer,
    struct swap_record_values *values)
{
    uint64_t payload;
    uint64_t commit;
    unsigned int *last = &context->last_generation[writer * context->page_count + page];

    if (values->generation > context->rounds) {
        fprintf(
            stderr,
            "file_cache_lru_race: future generation page=%zu writer=%u generation=%" PRIu64 "\n",
            page,
            writer,
            values->generation);
        return -1;
    }
    payload = swap_test_payload(page, writer, (unsigned int)values->generation);
    commit = swap_test_commit(page, writer, (unsigned int)values->generation, payload);
    if (values->page_index != page || values->writer_id != writer || values->payload != payload ||
        values->payload_inverse != ~payload || values->commit != commit || values->generation < *last) {
        fprintf(
            stderr,
            "file_cache_lru_race: invalid live record page=%zu writer=%u generation=%" PRIu64
            " last=%u commit=%016" PRIx64 "\n",
            page,
            writer,
            values->generation,
            *last,
            values->commit);
        return -1;
    }

    *last = (unsigned int)values->generation;
    return 0;
}

static void *file_writer(void *argument)
{
    struct writer_context *writer_context = argument;
    struct file_race_context *context = writer_context->race;
    unsigned int writer = writer_context->writer;

    for (unsigned int generation = 1; generation <= context->rounds; generation++) {
        for (size_t step = 0; step < context->page_count; step++) {
            size_t page = writer == 0 ? step : context->page_count - step - 1;
            int error;

            if ((page & 1U) != 0) {
                continue;
            }

            if (writer == 0) {
                error = pwrite_record(context->fd, page, context->page_size, writer, generation);
            } else {
                swap_test_store_record(
                    swap_test_record(context->write_alias, page, context->page_size, writer),
                    page,
                    writer,
                    generation);
                error = 0;
            }
            if (error != 0) {
                fprintf(
                    stderr,
                    "file_cache_lru_race: writer %u failed page=%zu: %s\n",
                    writer,
                    page,
                    strerror(errno));
                atomic_store_explicit(&context->failed, 1, memory_order_relaxed);
                atomic_fetch_add_explicit(&context->writers_done, 1, memory_order_release);
                return NULL;
            }
        }
        printf("file_cache_lru_race: writer %u generation %u/%u complete\n", writer, generation, context->rounds);
        fflush(stdout);
    }

    atomic_fetch_add_explicit(&context->writers_done, 1, memory_order_release);
    return NULL;
}

static void *alias_reader(void *argument)
{
    struct file_race_context *context = argument;

    while (atomic_load_explicit(&context->writers_done, memory_order_acquire) < WRITER_COUNT) {
        for (size_t page = 0; page < context->page_count; page++) {
            uint64_t tail = atomic_load_explicit(
                swap_test_tail(context->read_alias, page, context->page_size), memory_order_acquire);

            for (unsigned int writer = 0; writer < WRITER_COUNT; writer++) {
                struct swap_record_values values;

                if (swap_test_load_record(
                        swap_test_record(context->read_alias, page, context->page_size, writer), &values) == 0 &&
                    validate_live_record(context, page, writer, &values) != 0) {
                    atomic_store_explicit(&context->failed, 1, memory_order_relaxed);
                    return NULL;
                }
            }
            if (tail != swap_test_tail_canary(page)) {
                fprintf(stderr, "file_cache_lru_race: tail mismatch page=%zu\n", page);
                atomic_store_explicit(&context->failed, 1, memory_order_relaxed);
                return NULL;
            }

        }
    }
    return NULL;
}

static void *fsync_worker(void *argument)
{
    struct file_race_context *context = argument;
    unsigned int passes = 0;

    do {
        if (fsync(context->fd) != 0) {
            fprintf(stderr, "file_cache_lru_race: fsync failed: %s\n", strerror(errno));
            atomic_store_explicit(&context->failed, 1, memory_order_relaxed);
            return NULL;
        }
        passes++;
        if ((passes & 31U) == 0) {
            printf("file_cache_lru_race: fsync pass %u complete\n", passes);
            fflush(stdout);
        }
        sched_yield();
    } while (atomic_load_explicit(&context->writers_done, memory_order_acquire) < WRITER_COUNT);

    return NULL;
}

static void *pressure_worker(void *argument)
{
    struct file_race_context *context = argument;
    unsigned int generation = 2;

    while (!atomic_load_explicit(&context->stop_pressure, memory_order_relaxed)) {
        for (size_t page = 0; page < context->pressure_page_count; page++) {
            if (atomic_load_explicit(&context->stop_pressure, memory_order_relaxed)) {
                return NULL;
            }
            context->pressure[page * context->page_size] = (uint8_t)(page ^ (generation * 0x5bU));
            if ((page + 1) % SWAP_TEST_PROGRESS_PAGES == 0 || page + 1 == context->pressure_page_count) {
                printf(
                    "file_cache_lru_race: pressure pass %u: %zu/%zu pages\n",
                    generation,
                    page + 1,
                    context->pressure_page_count);
                fflush(stdout);
            }
        }
        printf("file_cache_lru_race: pressure pass %u complete\n", generation);
        fflush(stdout);
        generation++;
    }
    return NULL;
}

static int verify_disk(
    int fd,
    size_t page_count,
    size_t page_size,
    unsigned int generation,
    uint8_t *buffer)
{
    for (size_t page = 0; page < page_count; page++) {
        ssize_t length = pread(fd, buffer, page_size, (off_t)(page * page_size));

        if (length != (ssize_t)page_size) {
            fprintf(stderr, "file_cache_lru_race: pread failed: %s\n", length < 0 ? strerror(errno) : "short read");
            return -1;
        }
        for (unsigned int writer = 0; writer < WRITER_COUNT; writer++) {
            struct swap_record_values values;
            size_t offset = writer == 0 ? 0 : page_size / 2;

            memcpy(&values, buffer + offset, sizeof(values));
            if (swap_test_validate_values("file_cache_lru_race", &values, page, writer, generation) != 0) {
                return -1;
            }
        }
        uint64_t tail;
        memcpy(&tail, buffer + page_size - sizeof(tail), sizeof(tail));
        if (tail != swap_test_tail_canary(page)) {
            fprintf(stderr, "file_cache_lru_race: disk tail mismatch page=%zu\n", page);
            return -1;
        }

        if ((page + 1) % SWAP_TEST_PROGRESS_PAGES == 0 || page + 1 == page_count) {
            printf("file_cache_lru_race: disk verify: %zu/%zu pages\n", page + 1, page_count);
            fflush(stdout);
        }
    }
    return 0;
}

static void watchdog_handler(int signal_number)
{
    (void)signal_number;
    _exit(124);
}

int main(int argc, char **argv)
{
    const char *path = "/file_cache_lru_race.data";
    unsigned int rounds = DEFAULT_ROUNDS;
    size_t target_size = SWAP_TEST_DEFAULT_TARGET_MIB * SWAP_TEST_MIB;
    size_t pressure_extra = SWAP_TEST_DEFAULT_PRESSURE_MIB * SWAP_TEST_MIB;
    size_t page_size = swap_test_page_size("file_cache_lru_race");
    size_t pressure_size;
    size_t page_count;
    uint8_t *read_alias = MAP_FAILED;
    uint8_t *write_alias = MAP_FAILED;
    volatile uint8_t *pressure = MAP_FAILED;
    uint8_t *disk_buffer = NULL;
    unsigned int *last_generation = NULL;
    pthread_t writer_threads[WRITER_COUNT];
    pthread_t reader_thread;
    pthread_t fsync_thread;
    pthread_t pressure_thread;
    struct writer_context writer_contexts[WRITER_COUNT];
    struct file_race_context context;
    int writers_started = 0;
    int reader_started = 0;
    int fsync_started = 0;
    int pressure_started = 0;
    int fd = -1;
    int result = 1;

    if (argc > 4) {
        fprintf(stderr, "usage: %s [rounds] [target_mib] [pressure_extra_mib]\n", argv[0]);
        return 2;
    }

    swap_test_require_lock_free_atomics("file_cache_lru_race");
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
    page_count = target_size / page_size;
    printf(
        "file_cache_lru_race: file=%zu MiB pressure=%zu MiB pages=%zu rounds=%u\n",
        target_size / SWAP_TEST_MIB,
        pressure_size / SWAP_TEST_MIB,
        page_count,
        rounds);

    signal(SIGALRM, watchdog_handler);
    alarm(WATCHDOG_SECONDS);

    fd = open(path, O_RDWR | O_CREAT | O_TRUNC, 0600);
    if (fd < 0 || ftruncate(fd, (off_t)target_size) != 0) {
        fprintf(stderr, "file_cache_lru_race: file setup failed: %s\n", strerror(errno));
        goto cleanup;
    }
    read_alias = mmap(NULL, target_size, PROT_READ | PROT_WRITE, MAP_SHARED, fd, 0);
    write_alias = mmap(NULL, target_size, PROT_READ | PROT_WRITE, MAP_SHARED, fd, 0);
    pressure = mmap(NULL, pressure_size, PROT_READ | PROT_WRITE, MAP_PRIVATE | MAP_ANONYMOUS, -1, 0);
    disk_buffer = malloc(page_size);
    last_generation = calloc(WRITER_COUNT * page_count, sizeof(*last_generation));
    if (read_alias == MAP_FAILED || write_alias == MAP_FAILED || pressure == MAP_FAILED || disk_buffer == NULL ||
        last_generation == NULL) {
        fprintf(stderr, "file_cache_lru_race: allocation failed: %s\n", strerror(errno));
        goto cleanup;
    }

    swap_test_initialize_mapping(write_alias, page_count, page_size);
    if (fsync(fd) != 0) {
        fprintf(stderr, "file_cache_lru_race: initial fsync failed: %s\n", strerror(errno));
        goto cleanup;
    }
    swap_test_touch_pressure(
        pressure, pressure_size / page_size, page_size, 1, "file_cache_lru_race");

    context = (struct file_race_context){
        .fd = fd,
        .read_alias = read_alias,
        .write_alias = write_alias,
        .pressure = pressure,
        .page_count = page_count,
        .page_size = page_size,
        .pressure_page_count = pressure_size / page_size,
        .rounds = rounds,
        .last_generation = last_generation,
    };
    atomic_init(&context.writers_done, 0);
    atomic_init(&context.stop_pressure, 0);
    atomic_init(&context.failed, 0);

    if (pthread_create(&pressure_thread, NULL, pressure_worker, &context) == 0) {
        pressure_started = 1;
    }
    if (pressure_started && pthread_create(&fsync_thread, NULL, fsync_worker, &context) == 0) {
        fsync_started = 1;
    }
    if (fsync_started && pthread_create(&reader_thread, NULL, alias_reader, &context) == 0) {
        reader_started = 1;
    }
    for (unsigned int writer = 0; reader_started && writer < WRITER_COUNT; writer++) {
        writer_contexts[writer] = (struct writer_context){
            .race = &context,
            .writer = writer,
        };
        if (pthread_create(&writer_threads[writer], NULL, file_writer, &writer_contexts[writer]) != 0) {
            break;
        }
        writers_started++;
    }

    if (writers_started != WRITER_COUNT) {
        fprintf(stderr, "file_cache_lru_race: failed to create all workers\n");
        atomic_store_explicit(&context.failed, 1, memory_order_relaxed);
        atomic_store_explicit(&context.writers_done, WRITER_COUNT, memory_order_release);
    }
    for (int writer = 0; writer < writers_started; writer++) {
        pthread_join(writer_threads[writer], NULL);
    }
    if (reader_started) {
        pthread_join(reader_thread, NULL);
    }
    if (fsync_started) {
        pthread_join(fsync_thread, NULL);
    }
    if (pressure_started) {
        atomic_store_explicit(&context.stop_pressure, 1, memory_order_relaxed);
        pthread_join(pressure_thread, NULL);
        pressure_started = 0;
    }

    for (size_t page = 1; page < page_count; page += 2) {
        if (pwrite_record(fd, page, page_size, 0, rounds) != 0) {
            fprintf(stderr, "file_cache_lru_race: final pwrite failed page=%zu: %s\n", page, strerror(errno));
            goto cleanup;
        }
        swap_test_store_record(swap_test_record(write_alias, page, page_size, 1), page, 1, rounds);
    }

    if (writers_started != WRITER_COUNT || atomic_load_explicit(&context.failed, memory_order_relaxed) ||
        swap_test_verify_mapping("file_cache_lru_race", read_alias, page_count, page_size, rounds, 0) != 0 ||
        swap_test_verify_mapping("file_cache_lru_race", write_alias, page_count, page_size, rounds, 1) != 0 ||
        fsync(fd) != 0) {
        goto cleanup;
    }

    if (munmap(read_alias, target_size) != 0 || munmap(write_alias, target_size) != 0) {
        fprintf(stderr, "file_cache_lru_race: munmap failed: %s\n", strerror(errno));
        goto cleanup;
    }
    read_alias = MAP_FAILED;
    write_alias = MAP_FAILED;
    swap_test_touch_pressure(
        pressure, pressure_size / page_size, page_size, rounds + 2, "file_cache_lru_race");
    swap_test_touch_pressure(
        pressure, pressure_size / page_size, page_size, rounds + 3, "file_cache_lru_race");
    if (verify_disk(fd, page_count, page_size, rounds, disk_buffer) != 0) {
        goto cleanup;
    }

    result = 0;

cleanup:
    if (pressure_started) {
        atomic_store_explicit(&context.stop_pressure, 1, memory_order_relaxed);
        pthread_join(pressure_thread, NULL);
    }
    free(last_generation);
    free(disk_buffer);
    if (pressure != MAP_FAILED && munmap((void *)pressure, pressure_size) != 0) {
        result = 1;
    }
    if (write_alias != MAP_FAILED && munmap(write_alias, target_size) != 0) {
        result = 1;
    }
    if (read_alias != MAP_FAILED && munmap(read_alias, target_size) != 0) {
        result = 1;
    }
    if (fd >= 0 && close(fd) != 0) {
        result = 1;
    }
    if (fd >= 0 && unlink(path) != 0) {
        result = 1;
    }
    alarm(0);
    if (result == 0) {
        puts("file_cache_lru_race: PASS");
    }
    return result;
}
