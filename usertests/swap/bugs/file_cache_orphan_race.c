#define _GNU_SOURCE

/*
 * Orphan file page race — reproduces docs/race/swap.md §4.3 (P0).
 *
 * A logical file page is allowed to gain two distinct identities when a
 * pwrite that cloned a stale `Arc<FileSwappableFrame>` races against reclaim
 * deleting the canonical mapping entry. The exact sequence exercised each
 * round, per the design document:
 *
 *   1. Warm up and dirty many file pages (the whole target).
 *   2. Unmap every MAP_SHARED alias so mapping_refs == 0.
 *   3. Apply memory pressure so reclaim performs blocking writeback on the
 *      dirty file pages; concurrently another worker pwrite()s the same pages.
 *   4. cached_page() (src/.../file/mapping.rs:104) clones the stale Arc, then
 *      is_invalid() takes the page lock and waits for reclaim.
 *   5. Reclaim finishes writeback, marks the page Out and removes the
 *      canonical entry (src/.../swappable.rs:560).
 *   6. The woken pwrite resurrects the stale page and writes through it.
 *   7. After pwrite returns, re-establish a MAP_SHARED alias and fault. If the
 *      freshly committed generation cannot be read back, the stale write landed
 *      on an orphan frame that is no longer canonical — the bug is proven.
 *
 * Only a single lane per page is used: the writer is pwrite, the reader is the
 * post-remap fault. The invariant under test is "the last committed generation
 * is observable after re-establishing the mapping", which fails precisely when
 * an orphan page shadowed the canonical cache.
 */

#include "swap_test_common.h"

#include <fcntl.h>
#include <pthread.h>
#include <signal.h>
#include <stddef.h>
#include <sys/stat.h>

enum {
    DEFAULT_ROUNDS = 8,
    MAX_ROUNDS = 64,
    PRESSURE_PASSES = 2,
    WATCHDOG_SECONDS = 600,
    WRITER_ID = 0,
};

_Static_assert(sizeof(struct swap_page_record) == sizeof(struct swap_record_values), "record layout mismatch");

struct orphan_context {
    int fd;
    const char *path;
    size_t page_count;
    size_t page_size;
    unsigned int rounds;
    volatile uint8_t *pressure;
    size_t pressure_page_count;
    _Atomic unsigned int round_generation;
    _Atomic unsigned int round_done;
    _Atomic int stop_pressure;
    _Atomic int pwrite_failed;
    _Atomic int disk_failed;
};

static off_t record_offset(size_t page, size_t page_size)
{
    return (off_t)(page * page_size);
}

static int pwrite_all(int fd, const void *buffer, size_t length, off_t offset)
{
    const uint8_t *bytes = buffer;

    while (length != 0) {
        ssize_t written = pwrite(fd, bytes, length, offset);
        if (written < 0 && errno == EINTR) {
            continue;
        }
        if (written < 0) {
            return -1;
        }
        if (written == 0) {
            return -1;
        }
        bytes += written;
        length -= (size_t)written;
        offset += written;
    }
    return 0;
}

static int pwrite_round(int fd, size_t page_count, size_t page_size, unsigned int generation)
{
    for (size_t page = 0; page < page_count; page++) {
        struct swap_record_values values = swap_test_record_values(page, WRITER_ID, generation);
        off_t offset = record_offset(page, page_size);
        off_t commit_offset = offset + (off_t)offsetof(struct swap_record_values, commit);
        uint64_t zeroed_commit = 0;

        if (pwrite_all(fd, &zeroed_commit, sizeof(zeroed_commit), commit_offset) != 0 ||
            pwrite_all(fd, &values, offsetof(struct swap_record_values, commit), offset) != 0 ||
            pwrite_all(fd, &values.commit, sizeof(values.commit), commit_offset) != 0) {
            return -1;
        }
    }
    return 0;
}

static int verify_disk(int fd, size_t page_count, size_t page_size, unsigned int generation, uint8_t *buffer)
{
    for (size_t page = 0; page < page_count; page++) {
        struct swap_record_values values;
        uint64_t tail = 0;
        off_t offset = record_offset(page, page_size);
        ssize_t length = pread(fd, buffer, page_size, offset);

        if (length < 0) {
            fprintf(stderr, "file_cache_orphan_race: pread failed page=%zu: %s\n", page, strerror(errno));
            return -1;
        }
        if ((size_t)length != page_size) {
            fprintf(stderr, "file_cache_orphan_race: short pread page=%zu len=%zd\n", page, length);
            return -1;
        }
        memcpy(&values, buffer, sizeof(values));
        if (swap_test_validate_values("file_cache_orphan_race", &values, page, WRITER_ID, generation) != 0) {
            return -1;
        }
        memcpy(&tail, buffer + page_size - sizeof(tail), sizeof(tail));
        if (tail != swap_test_tail_canary(page)) {
            fprintf(
                stderr,
                "file_cache_orphan_race: tail mismatch page=%zu expected=%016" PRIx64 " actual=%016" PRIx64 "\n",
                page,
                swap_test_tail_canary(page),
                tail);
            return -1;
        }
    }
    return 0;
}

static void store_tail_canaries(uint8_t *mapping, size_t page_count, size_t page_size)
{
    for (size_t page = 0; page < page_count; page++) {
        atomic_store_explicit(
            swap_test_tail(mapping, page, page_size),
            swap_test_tail_canary(page),
            memory_order_release);
    }
}

/*
 * Pressure worker. Repeatedly touches the shared pressure region to keep the
 * swapper busy and force foreground reclaim against the dirty file pages.
 */
static void *pressure_worker(void *argument)
{
    struct orphan_context *context = argument;
    unsigned int generation = 1;

    while (!atomic_load_explicit(&context->stop_pressure, memory_order_relaxed)) {
        swap_test_touch_pressure(
            context->pressure,
            context->pressure_page_count,
            context->page_size,
            generation,
            "file_cache_orphan_race");
        generation++;
    }
    return NULL;
}

/*
 * Pwrite worker. Spins on the current round generation, publishes a full
 * generation via pwrite (which walks cached_page -> ensure_page, the window
 * documented at mapping.rs:104), then signals completion.
 *
 * pwrite is the only writer to the file pages while every MAP_SHARED alias is
 * torn down, so mapping_refs == 0 holds for the entire window.
 */
static void *pwrite_worker(void *argument)
{
    struct orphan_context *context = argument;

    for (unsigned int round = 1; round <= context->rounds; round++) {
        while (atomic_load_explicit(&context->round_generation, memory_order_acquire) != round) {
            sched_yield();
        }
        if (pwrite_round(context->fd, context->page_count, context->page_size, round) != 0) {
            fprintf(stderr, "file_cache_orphan_race: pwrite failed round=%u: %s\n", round, strerror(errno));
            atomic_store_explicit(&context->pwrite_failed, 1, memory_order_relaxed);
            atomic_store_explicit(&context->round_done, round, memory_order_release);
            return NULL;
        }
        if (fsync(context->fd) != 0) {
            fprintf(stderr, "file_cache_orphan_race: fsync failed round=%u: %s\n", round, strerror(errno));
            atomic_store_explicit(&context->pwrite_failed, 1, memory_order_relaxed);
            atomic_store_explicit(&context->round_done, round, memory_order_release);
            return NULL;
        }
        atomic_store_explicit(&context->round_done, round, memory_order_release);
        if ((round & 3U) == 0 || round == context->rounds) {
            printf("file_cache_orphan_race: pwrite round %u/%u done\n", round, context->rounds);
            fflush(stdout);
        }
    }
    return NULL;
}

static void watchdog_handler(int signal_number)
{
    (void)signal_number;
    _exit(2);
}

int main(int argc, char **argv)
{
    static const char *test_name = "file_cache_orphan_race";
    const char *path = "/file_cache_orphan_race.data";
    unsigned int rounds = DEFAULT_ROUNDS;
    size_t target_size = SWAP_TEST_DEFAULT_TARGET_MIB * SWAP_TEST_MIB;
    size_t pressure_extra = SWAP_TEST_DEFAULT_PRESSURE_MIB * SWAP_TEST_MIB;
    size_t page_size = swap_test_page_size(test_name);
    size_t pressure_size;
    size_t page_count;
    uint8_t *shared_alias = MAP_FAILED;
    uint8_t *verify_alias = MAP_FAILED;
    volatile uint8_t *pressure = MAP_FAILED;
    uint8_t *disk_buffer = NULL;
    pthread_t pressure_thread;
    pthread_t pwrite_thread;
    int pressure_started = 0;
    int pwrite_started = 0;
    int fd = -1;
    int result = 1;
    struct orphan_context context;

    if (argc > 4) {
        fprintf(stderr, "usage: %s [rounds] [target_mib] [pressure_extra_mib]\n", argv[0]);
        return 2;
    }
    swap_test_require_lock_free_atomics(test_name);
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
        "%s: file=%zu MiB pressure=%zu MiB pages=%zu rounds=%u\n",
        test_name,
        target_size / SWAP_TEST_MIB,
        pressure_size / SWAP_TEST_MIB,
        page_count,
        rounds);

    signal(SIGALRM, watchdog_handler);
    alarm(WATCHDOG_SECONDS);

    fd = open(path, O_RDWR | O_CREAT | O_TRUNC, 0600);
    if (fd < 0 || ftruncate(fd, (off_t)target_size) != 0) {
        fprintf(stderr, "%s: file setup failed: %s\n", test_name, strerror(errno));
        goto cleanup;
    }

    /*
     * Step 1: warm up and dirty the whole file via a MAP_SHARED alias. After
     * this fsync the pages are resident, dirty-flag-clear, and cached with
     * mapping_refs > 0 thanks to the alias.
     */
    shared_alias = mmap(NULL, target_size, PROT_READ | PROT_WRITE, MAP_SHARED, fd, 0);
    if (shared_alias == MAP_FAILED) {
        fprintf(stderr, "%s: warmup mmap failed: %s\n", test_name, strerror(errno));
        goto cleanup;
    }
    swap_test_initialize_mapping(shared_alias, page_count, page_size);
    if (fsync(fd) != 0) {
        fprintf(stderr, "%s: warmup fsync failed: %s\n", test_name, strerror(errno));
        goto cleanup;
    }

    pressure = mmap(NULL, pressure_size, PROT_READ | PROT_WRITE, MAP_PRIVATE | MAP_ANONYMOUS, -1, 0);
    disk_buffer = malloc(page_size);
    if (pressure == MAP_FAILED || disk_buffer == NULL) {
        fprintf(stderr, "%s: allocation failed: %s\n", test_name, strerror(errno));
        goto cleanup;
    }

    /*
     * Step 2: drop every MAP_SHARED alias so mapping_refs reaches 0 for every
     * file page. The cached frames stay resident (file backend retains
     * unmapped pages), which is exactly the precondition reclaim needs to
     * remove the canonical mapping entry.
     */
    if (munmap(shared_alias, target_size) != 0) {
        fprintf(stderr, "%s: munmap alias failed: %s\n", test_name, strerror(errno));
        goto cleanup;
    }
    shared_alias = MAP_FAILED;

    /* Dirty the now-unmapped cached pages again via pwrite so reclaim has
     * blocking writeback work to do during step 3. */
    if (pwrite_round(fd, page_count, page_size, 0) != 0 || fsync(fd) != 0) {
        fprintf(stderr, "%s: post-warmup pwrite failed: %s\n", test_name, strerror(errno));
        goto cleanup;
    }
    /* Re-dirty without fsync so the cached pages are software-dirty. */
    if (pwrite_round(fd, page_count, page_size, 0) != 0) {
        fprintf(stderr, "%s: redirty pwrite failed: %s\n", test_name, strerror(errno));
        goto cleanup;
    }

    context = (struct orphan_context){
        .fd = fd,
        .path = path,
        .page_count = page_count,
        .page_size = page_size,
        .rounds = rounds,
        .pressure = pressure,
        .pressure_page_count = pressure_size / page_size,
    };
    atomic_init(&context.round_generation, 0);
    atomic_init(&context.round_done, 0);
    atomic_init(&context.stop_pressure, 0);
    atomic_init(&context.pwrite_failed, 0);
    atomic_init(&context.disk_failed, 0);

    if (pthread_create(&pressure_thread, NULL, pressure_worker, &context) != 0) {
        fprintf(stderr, "%s: pressure thread failed: %s\n", test_name, strerror(errno));
        goto cleanup;
    }
    pressure_started = 1;
    if (pthread_create(&pwrite_thread, NULL, pwrite_worker, &context) != 0) {
        fprintf(stderr, "%s: pwrite thread failed: %s\n", test_name, strerror(errno));
        goto cleanup;
    }
    pwrite_started = 1;

    /*
     * Steps 3..6: drive the race. For each round, first apply pressure so
     * reclaim is actively walking the file pages, then release the pwrite
     * worker on that round's generation. The pwrite worker blocks in
     * cached_page()/ensure_page while reclaim holds the page lock for
     * writeback; when reclaim finishes it sets Out and removes the canonical
     * entry, after which the woken pwrite resurrects the stale Arc and writes
     * through an orphan frame.
     */
    for (unsigned int round = 1; round <= rounds; round++) {
        for (unsigned int pass = 0; pass < PRESSURE_PASSES; pass++) {
            swap_test_touch_pressure(
                pressure,
                pressure_size / page_size,
                page_size,
                round * PRESSURE_PASSES + pass,
                test_name);
        }
        atomic_store_explicit(&context.round_generation, round, memory_order_release);
        while (atomic_load_explicit(&context.round_done, memory_order_acquire) != round) {
            sched_yield();
        }
        if (atomic_load_explicit(&context.pwrite_failed, memory_order_relaxed)) {
            goto cleanup;
        }

        /*
         * Step 7: re-establish a MAP_SHARED alias and fault every page. If the
         * committed generation for this round is not fully visible, an orphan
         * page shadowed the canonical cache. Verify both the live alias and the
         * on-disk content.
         */
        verify_alias = mmap(NULL, target_size, PROT_READ | PROT_WRITE, MAP_SHARED, fd, 0);
        if (verify_alias == MAP_FAILED) {
            fprintf(stderr, "%s: verify mmap round=%u failed: %s\n", test_name, round, strerror(errno));
            goto cleanup;
        }
        store_tail_canaries(verify_alias, page_count, page_size);
        if (swap_test_verify_mapping(test_name, verify_alias, page_count, page_size, round, 0) != 0) {
            fprintf(stderr, "%s: orphan detected round=%u (alias mismatch)\n", test_name, round);
            goto cleanup;
        }
        if (verify_disk(fd, page_count, page_size, round, disk_buffer) != 0) {
            fprintf(stderr, "%s: orphan detected round=%u (disk mismatch)\n", test_name, round);
            atomic_store_explicit(&context.disk_failed, 1, memory_order_relaxed);
            goto cleanup;
        }
        if (munmap(verify_alias, target_size) != 0) {
            fprintf(stderr, "%s: verify munmap round=%u failed: %s\n", test_name, round, strerror(errno));
            goto cleanup;
        }
        verify_alias = MAP_FAILED;
        printf("%s: round %u/%u verified\n", test_name, round, rounds);
        fflush(stdout);
    }

    result = 0;

cleanup:
    atomic_store_explicit(&context.stop_pressure, 1, memory_order_relaxed);
    if (pwrite_started) {
        atomic_store_explicit(&context.round_generation, rounds + 1, memory_order_release);
        pthread_join(pwrite_thread, NULL);
    }
    if (pressure_started) {
        pthread_join(pressure_thread, NULL);
    }
    free(disk_buffer);
    if (verify_alias != MAP_FAILED && munmap(verify_alias, target_size) != 0) {
        result = 1;
    }
    if (shared_alias != MAP_FAILED && munmap(shared_alias, target_size) != 0) {
        result = 1;
    }
    if (pressure != MAP_FAILED && munmap((void *)pressure, pressure_size) != 0) {
        result = 1;
    }
    if (fd >= 0 && close(fd) != 0) {
        result = 1;
    }
    if (fd >= 0 && unlink(path) != 0) {
        result = 1;
    }
    alarm(0);
    if (result == 0 && !atomic_load_explicit(&context.disk_failed, memory_order_relaxed) &&
        !atomic_load_explicit(&context.pwrite_failed, memory_order_relaxed)) {
        printf("%s: PASS\n", test_name);
    }
    return result;
}
