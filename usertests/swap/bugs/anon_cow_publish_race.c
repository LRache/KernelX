#define _GNU_SOURCE

/* Concurrent COW publication, observer-write preservation, and swap pressure. */

#include "swap_test_common.h"

#include <pthread.h>
#include <signal.h>
#include <sys/wait.h>

enum {
    DEFAULT_ROUNDS = 8,
    MAX_ROUNDS = 64,
    OBSERVER_WRITES_PER_PAGE = 256,
    WATCHDOG_SECONDS = 300,
};

#define NO_ACTIVE_PAGE SIZE_MAX

struct cow_context {
    uint8_t *mapping;
    size_t *expected_counts;
    size_t page_count;
    size_t page_size;
    unsigned int generation;
    _Atomic size_t active_page;
    _Atomic size_t observer_ready;
    _Atomic size_t observer_done;
    _Atomic int failed;
    _Atomic int abort;
};

struct pressure_context {
    volatile uint8_t *mapping;
    size_t page_count;
    size_t page_size;
    _Atomic int stop;
    _Atomic unsigned int passes;
};

static _Atomic uint64_t *cow_start_canary(uint8_t *base, size_t page, size_t page_size)
{
    return (_Atomic uint64_t *)(base + page * page_size);
}

static _Atomic uint64_t *cow_writer_generation(uint8_t *base, size_t page, size_t page_size)
{
    return (_Atomic uint64_t *)(base + page * page_size + page_size / 2);
}

static _Atomic uint64_t *cow_observer_count(uint8_t *base, size_t page, size_t page_size)
{
    return cow_writer_generation(base, page, page_size) + 1;
}

static _Atomic uint64_t *cow_tail_canary(uint8_t *base, size_t page, size_t page_size)
{
    return (_Atomic uint64_t *)(base + (page + 1) * page_size - sizeof(uint64_t));
}

static uint64_t cow_canary(size_t page, uint64_t salt)
{
    return salt ^ ((uint64_t)page * UINT64_C(0x9e3779b97f4a7c15));
}

static void initialize_mapping(uint8_t *base, size_t page_count, size_t page_size)
{
    for (size_t page = 0; page < page_count; page++) {
        atomic_store_explicit(
            cow_start_canary(base, page, page_size),
            cow_canary(page, UINT64_C(0x6a09e667f3bcc909)),
            memory_order_relaxed);
        atomic_store_explicit(cow_writer_generation(base, page, page_size), 0, memory_order_relaxed);
        atomic_store_explicit(cow_observer_count(base, page, page_size), 0, memory_order_relaxed);
        atomic_store_explicit(
            cow_tail_canary(base, page, page_size),
            cow_canary(page, UINT64_C(0xbb67ae8584caa73b)),
            memory_order_release);
    }
}

static int verify_canaries(uint8_t *base, size_t page, size_t page_size, const char *who)
{
    uint64_t expected_start = cow_canary(page, UINT64_C(0x6a09e667f3bcc909));
    uint64_t expected_tail = cow_canary(page, UINT64_C(0xbb67ae8584caa73b));
    uint64_t start = atomic_load_explicit(cow_start_canary(base, page, page_size), memory_order_acquire);
    uint64_t tail = atomic_load_explicit(cow_tail_canary(base, page, page_size), memory_order_acquire);

    if (start == expected_start && tail == expected_tail) {
        return 0;
    }

    fprintf(
        stderr,
        "anon_cow_publish_race: %s canary mismatch page=%zu start=%016" PRIx64 "/%016" PRIx64
        " tail=%016" PRIx64 "/%016" PRIx64 "\n",
        who,
        page,
        start,
        expected_start,
        tail,
        expected_tail);
    return -1;
}

static int verify_child_snapshot(uint8_t *base, size_t page_count, size_t page_size)
{
    for (size_t page = 0; page < page_count; page++) {
        uint64_t writer = atomic_load_explicit(cow_writer_generation(base, page, page_size), memory_order_acquire);
        uint64_t observer = atomic_load_explicit(cow_observer_count(base, page, page_size), memory_order_acquire);

        if (verify_canaries(base, page, page_size, "child") != 0 || writer != 0 || observer != 0) {
            fprintf(
                stderr,
                "anon_cow_publish_race: child COW isolation failure page=%zu writer=%" PRIu64
                " observer=%" PRIu64 "\n",
                page,
                writer,
                observer);
            return -1;
        }
    }
    return 0;
}

static int verify_parent_result(const struct cow_context *context)
{
    for (size_t page = 0; page < context->page_count; page++) {
        uint64_t writer = atomic_load_explicit(
            cow_writer_generation(context->mapping, page, context->page_size), memory_order_acquire);
        uint64_t observer = atomic_load_explicit(
            cow_observer_count(context->mapping, page, context->page_size), memory_order_acquire);

        if (verify_canaries(context->mapping, page, context->page_size, "parent") != 0 ||
            writer != context->generation || observer != context->expected_counts[page]) {
            fprintf(
                stderr,
                "anon_cow_publish_race: parent result mismatch page=%zu writer=%" PRIu64 "/%u"
                " observer=%" PRIu64 "/%zu\n",
                page,
                writer,
                context->generation,
                observer,
                context->expected_counts[page]);
            return -1;
        }
    }
    return 0;
}

static void *cow_writer(void *argument)
{
    struct cow_context *context = argument;

    for (size_t page = 0; page < context->page_count; page++) {
        atomic_store_explicit(&context->active_page, page, memory_order_release);
        while (atomic_load_explicit(&context->observer_ready, memory_order_acquire) != page + 1) {
            if (atomic_load_explicit(&context->abort, memory_order_relaxed)) {
                return NULL;
            }
            sched_yield();
        }

        atomic_store_explicit(
            cow_writer_generation(context->mapping, page, context->page_size),
            context->generation,
            memory_order_release);
        sched_yield();
        atomic_store_explicit(&context->active_page, NO_ACTIVE_PAGE, memory_order_release);

        while (atomic_load_explicit(&context->observer_done, memory_order_acquire) != page + 1) {
            if (atomic_load_explicit(&context->abort, memory_order_relaxed)) {
                return NULL;
            }
            sched_yield();
        }

        if ((page + 1) % SWAP_TEST_PROGRESS_PAGES == 0 || page + 1 == context->page_count) {
            printf(
                "anon_cow_publish_race: generation %u COW: %zu/%zu pages\n",
                context->generation,
                page + 1,
                context->page_count);
            fflush(stdout);
        }
    }
    return NULL;
}

static void *cow_observer(void *argument)
{
    struct cow_context *context = argument;

    for (size_t page = 0; page < context->page_count; page++) {
        size_t writes = 0;

        while (atomic_load_explicit(&context->active_page, memory_order_acquire) != page) {
            if (atomic_load_explicit(&context->abort, memory_order_relaxed)) {
                return NULL;
            }
            sched_yield();
        }

        if (verify_canaries(context->mapping, page, context->page_size, "observer") != 0) {
            atomic_store_explicit(&context->failed, 1, memory_order_relaxed);
        }
        atomic_store_explicit(&context->observer_ready, page + 1, memory_order_release);

        do {
            if (verify_canaries(context->mapping, page, context->page_size, "observer") != 0) {
                atomic_store_explicit(&context->failed, 1, memory_order_relaxed);
            }
            atomic_fetch_add_explicit(cow_observer_count(context->mapping, page, context->page_size), 1, memory_order_acq_rel);
            writes++;
        } while (writes < OBSERVER_WRITES_PER_PAGE &&
                 atomic_load_explicit(&context->active_page, memory_order_acquire) == page);

        context->expected_counts[page] = writes;
        atomic_store_explicit(&context->observer_done, page + 1, memory_order_release);
    }
    return NULL;
}

static void *pressure_worker(void *argument)
{
    struct pressure_context *context = argument;
    unsigned int generation = 1;

    while (!atomic_load_explicit(&context->stop, memory_order_relaxed)) {
        for (size_t page = 0; page < context->page_count; page++) {
            if (atomic_load_explicit(&context->stop, memory_order_relaxed)) {
                return NULL;
            }
            context->mapping[page * context->page_size] = (uint8_t)(page ^ (generation * 0x5bU));
            if ((page + 1) % SWAP_TEST_PROGRESS_PAGES == 0 || page + 1 == context->page_count) {
                printf(
                    "anon_cow_publish_race: pressure pass %u: %zu/%zu pages\n",
                    generation,
                    page + 1,
                    context->page_count);
                fflush(stdout);
            }
        }
        atomic_fetch_add_explicit(&context->passes, 1, memory_order_release);
        printf("anon_cow_publish_race: pressure pass %u complete\n", generation);
        fflush(stdout);
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
    unsigned int rounds = DEFAULT_ROUNDS;
    size_t target_size = SWAP_TEST_DEFAULT_TARGET_MIB * SWAP_TEST_MIB;
    size_t pressure_extra = SWAP_TEST_DEFAULT_PRESSURE_MIB * SWAP_TEST_MIB;
    size_t page_size = swap_test_page_size("anon_cow_publish_race");
    size_t pressure_size;
    size_t page_count;
    uint8_t *mapping = MAP_FAILED;
    volatile uint8_t *pressure = MAP_FAILED;
    size_t *expected_counts = NULL;
    pthread_t pressure_thread;
    struct pressure_context pressure_context;
    int pressure_started = 0;
    int result = 1;

    if (argc > 4) {
        fprintf(stderr, "usage: %s [rounds] [target_mib] [pressure_extra_mib]\n", argv[0]);
        return 2;
    }
    swap_test_require_lock_free_atomics("anon_cow_publish_race");
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
        "anon_cow_publish_race: target=%zu MiB pressure=%zu MiB pages=%zu rounds=%u\n",
        target_size / SWAP_TEST_MIB,
        pressure_size / SWAP_TEST_MIB,
        page_count,
        rounds);

    signal(SIGALRM, watchdog_handler);
    signal(SIGPIPE, SIG_IGN);
    alarm(WATCHDOG_SECONDS);

    mapping = mmap(NULL, target_size, PROT_READ | PROT_WRITE, MAP_PRIVATE | MAP_ANONYMOUS, -1, 0);
    pressure = mmap(NULL, pressure_size, PROT_READ | PROT_WRITE, MAP_SHARED | MAP_ANONYMOUS, -1, 0);
    expected_counts = calloc(page_count, sizeof(*expected_counts));
    if (mapping == MAP_FAILED || pressure == MAP_FAILED || expected_counts == NULL) {
        fprintf(stderr, "anon_cow_publish_race: allocation failed: %s\n", strerror(errno));
        goto cleanup;
    }

    pressure_context = (struct pressure_context){
        .mapping = pressure,
        .page_count = pressure_size / page_size,
        .page_size = page_size,
    };
    atomic_init(&pressure_context.stop, 0);
    atomic_init(&pressure_context.passes, 0);

    for (unsigned int round = 0; round < rounds; round++) {
        struct cow_context context;
        pthread_t observer_thread;
        pthread_t writer_thread;
        int observer_started = 0;
        int writer_started = 0;
        int release_pipe[2] = {-1, -1};
        pid_t child;
        int status;
        char token = 'C';

        initialize_mapping(mapping, page_count, page_size);
        memset(expected_counts, 0, page_count * sizeof(*expected_counts));
        if (pipe(release_pipe) != 0) {
            fprintf(stderr, "anon_cow_publish_race: pipe failed: %s\n", strerror(errno));
            goto cleanup;
        }

        fflush(NULL);
        child = fork();
        if (child < 0) {
            fprintf(stderr, "anon_cow_publish_race: fork failed: %s\n", strerror(errno));
            close(release_pipe[0]);
            close(release_pipe[1]);
            goto cleanup;
        }
        if (child == 0) {
            close(release_pipe[1]);
            alarm(WATCHDOG_SECONDS);
            if (munmap((void *)pressure, pressure_size) != 0) {
                _exit(1);
            }
            if (read(release_pipe[0], &token, 1) != 1 || verify_child_snapshot(mapping, page_count, page_size) != 0) {
                _exit(1);
            }
            _exit(0);
        }

        close(release_pipe[0]);
        context = (struct cow_context){
            .mapping = mapping,
            .expected_counts = expected_counts,
            .page_count = page_count,
            .page_size = page_size,
            .generation = round + 1,
        };
        atomic_init(&context.active_page, NO_ACTIVE_PAGE);
        atomic_init(&context.observer_ready, 0);
        atomic_init(&context.observer_done, 0);
        atomic_init(&context.failed, 0);
        atomic_init(&context.abort, 0);

        atomic_store_explicit(&pressure_context.stop, 0, memory_order_relaxed);
        atomic_store_explicit(&pressure_context.passes, 0, memory_order_relaxed);
        if (pthread_create(&pressure_thread, NULL, pressure_worker, &pressure_context) != 0) {
            fprintf(stderr, "anon_cow_publish_race: failed to create pressure worker\n");
            kill(child, SIGKILL);
            close(release_pipe[1]);
            waitpid(child, NULL, 0);
            goto cleanup;
        }
        pressure_started = 1;
        while (atomic_load_explicit(&pressure_context.passes, memory_order_acquire) == 0) {
            sched_yield();
        }

        if (pthread_create(&observer_thread, NULL, cow_observer, &context) == 0) {
            observer_started = 1;
        }
        if (observer_started && pthread_create(&writer_thread, NULL, cow_writer, &context) == 0) {
            writer_started = 1;
        }
        if (!writer_started) {
            fprintf(stderr, "anon_cow_publish_race: failed to create COW workers\n");
            atomic_store_explicit(&context.abort, 1, memory_order_relaxed);
        }
        if (writer_started) {
            pthread_join(writer_thread, NULL);
        }
        if (observer_started) {
            pthread_join(observer_thread, NULL);
        }
        atomic_store_explicit(&pressure_context.stop, 1, memory_order_relaxed);
        pthread_join(pressure_thread, NULL);
        pressure_started = 0;

        if (write(release_pipe[1], &token, 1) != 1) {
            fprintf(stderr, "anon_cow_publish_race: child release failed: %s\n", strerror(errno));
            kill(child, SIGKILL);
        }
        close(release_pipe[1]);
        if (waitpid(child, &status, 0) < 0) {
            fprintf(stderr, "anon_cow_publish_race: waitpid failed: %s\n", strerror(errno));
            goto cleanup;
        }
        if (!writer_started || atomic_load_explicit(&context.failed, memory_order_relaxed) ||
            verify_parent_result(&context) != 0 || !WIFEXITED(status) || WEXITSTATUS(status) != 0) {
            fprintf(stderr, "anon_cow_publish_race: round %u failed, child status=0x%x\n", round + 1, status);
            goto cleanup;
        }

        printf("anon_cow_publish_race: round %u/%u complete\n", round + 1, rounds);
        fflush(stdout);
    }

    result = 0;

cleanup:
    if (pressure_started) {
        atomic_store_explicit(&pressure_context.stop, 1, memory_order_relaxed);
        pthread_join(pressure_thread, NULL);
    }
    free(expected_counts);
    if (pressure != MAP_FAILED && munmap((void *)pressure, pressure_size) != 0) {
        result = 1;
    }
    if (mapping != MAP_FAILED && munmap(mapping, target_size) != 0) {
        result = 1;
    }
    alarm(0);
    if (result == 0) {
        puts("anon_cow_publish_race: PASS");
    }
    return result;
}
