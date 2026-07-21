#ifndef USERTESTS_SWAP_TEST_COMMON_H
#define USERTESTS_SWAP_TEST_COMMON_H

#include <errno.h>
#include <inttypes.h>
#include <sched.h>
#include <stdatomic.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/mman.h>
#include <unistd.h>

enum {
    SWAP_TEST_DEFAULT_TARGET_MIB = 8,
    SWAP_TEST_DEFAULT_PRESSURE_MIB = 64,
    SWAP_TEST_FALLBACK_MEMORY_MIB = 1024,
    SWAP_TEST_PROGRESS_PAGES = 4096,
};

#define SWAP_TEST_MIB (1024UL * 1024UL)

struct swap_page_record {
    _Atomic uint64_t page_index;
    _Atomic uint64_t writer_id;
    _Atomic uint64_t generation;
    _Atomic uint64_t payload;
    _Atomic uint64_t payload_inverse;
    _Atomic uint64_t commit;
};

struct swap_record_values {
    uint64_t page_index;
    uint64_t writer_id;
    uint64_t generation;
    uint64_t payload;
    uint64_t payload_inverse;
    uint64_t commit;
};

static inline size_t swap_test_parse_mib(const char *arg, const char *description)
{
    char *end = NULL;
    unsigned long long mib;

    errno = 0;
    mib = strtoull(arg, &end, 0);
    if (errno != 0 || end == arg || *end != '\0' || mib == 0 || mib > SIZE_MAX / SWAP_TEST_MIB) {
        fprintf(stderr, "invalid %s in MiB: %s\n", description, arg);
        exit(2);
    }

    return (size_t)mib * SWAP_TEST_MIB;
}

static inline unsigned int swap_test_parse_count(const char *arg, const char *description, unsigned int maximum)
{
    char *end = NULL;
    unsigned long value;

    errno = 0;
    value = strtoul(arg, &end, 0);
    if (errno != 0 || end == arg || *end != '\0' || value == 0 || value > maximum) {
        fprintf(stderr, "invalid %s: %s (expected 1..%u)\n", description, arg, maximum);
        exit(2);
    }

    return (unsigned int)value;
}

static inline size_t swap_test_read_mem_total(void)
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

static inline size_t swap_test_page_size(const char *test_name)
{
    long value = sysconf(_SC_PAGESIZE);

    if (value <= 0) {
        fprintf(stderr, "%s: sysconf(_SC_PAGESIZE) failed: %s\n", test_name, strerror(errno));
        exit(1);
    }
    if ((size_t)value < 2 * sizeof(struct swap_page_record) + sizeof(uint64_t) ||
        (size_t)value % sizeof(uint64_t) != 0) {
        fprintf(stderr, "%s: unsupported page size %ld\n", test_name, value);
        exit(1);
    }

    return (size_t)value;
}

static inline size_t swap_test_pressure_size(size_t page_size, size_t pressure_extra)
{
    size_t memory_size = swap_test_read_mem_total();

    if (memory_size == 0) {
        memory_size = SWAP_TEST_FALLBACK_MEMORY_MIB * SWAP_TEST_MIB;
    }
    if (memory_size > SIZE_MAX - pressure_extra) {
        fprintf(stderr, "swap test pressure size overflow\n");
        exit(1);
    }

    return (memory_size + pressure_extra) / page_size * page_size;
}

static inline uint64_t swap_test_payload(size_t page, unsigned int writer, unsigned int generation)
{
    return UINT64_C(0x243f6a8885a308d3) ^ ((uint64_t)page * UINT64_C(0x9e3779b97f4a7c15)) ^
           ((uint64_t)writer << 40) ^ ((uint64_t)generation << 52);
}

static inline uint64_t swap_test_commit(size_t page, unsigned int writer, unsigned int generation, uint64_t payload)
{
    return UINT64_C(0x13198a2e03707344) ^ (uint64_t)page ^ ((uint64_t)writer << 32) ^
           ((uint64_t)generation << 48) ^ payload;
}

static inline uint64_t swap_test_tail_canary(size_t page)
{
    return UINT64_C(0xa4093822299f31d0) ^ ((uint64_t)page * UINT64_C(0xd6e8feb86659fd93));
}

static inline struct swap_record_values swap_test_record_values(
    size_t page,
    unsigned int writer,
    unsigned int generation)
{
    struct swap_record_values values;

    values.page_index = page;
    values.writer_id = writer;
    values.generation = generation;
    values.payload = swap_test_payload(page, writer, generation);
    values.payload_inverse = ~values.payload;
    values.commit = swap_test_commit(page, writer, generation, values.payload);
    return values;
}

static inline struct swap_page_record *swap_test_record(
    uint8_t *base,
    size_t page,
    size_t page_size,
    unsigned int writer)
{
    size_t offset = writer == 0 ? 0 : page_size / 2;
    return (struct swap_page_record *)(base + page * page_size + offset);
}

static inline _Atomic uint64_t *swap_test_tail(uint8_t *base, size_t page, size_t page_size)
{
    return (_Atomic uint64_t *)(base + (page + 1) * page_size - sizeof(uint64_t));
}

static inline void swap_test_store_record(
    struct swap_page_record *record,
    size_t page,
    unsigned int writer,
    unsigned int generation)
{
    struct swap_record_values values = swap_test_record_values(page, writer, generation);

    atomic_store_explicit(&record->commit, 0, memory_order_release);
    atomic_store_explicit(&record->page_index, values.page_index, memory_order_relaxed);
    atomic_store_explicit(&record->writer_id, values.writer_id, memory_order_relaxed);
    atomic_store_explicit(&record->generation, values.generation, memory_order_relaxed);
    atomic_store_explicit(&record->payload, values.payload, memory_order_relaxed);
    atomic_store_explicit(&record->payload_inverse, values.payload_inverse, memory_order_relaxed);
    atomic_store_explicit(&record->commit, values.commit, memory_order_release);
}

static inline int swap_test_load_record(struct swap_page_record *record, struct swap_record_values *values)
{
    for (unsigned int attempt = 0; attempt < 16; attempt++) {
        uint64_t before = atomic_load_explicit(&record->commit, memory_order_acquire);

        if (before == 0) {
            sched_yield();
            continue;
        }
        values->page_index = atomic_load_explicit(&record->page_index, memory_order_relaxed);
        values->writer_id = atomic_load_explicit(&record->writer_id, memory_order_relaxed);
        values->generation = atomic_load_explicit(&record->generation, memory_order_relaxed);
        values->payload = atomic_load_explicit(&record->payload, memory_order_relaxed);
        values->payload_inverse = atomic_load_explicit(&record->payload_inverse, memory_order_relaxed);
        values->commit = atomic_load_explicit(&record->commit, memory_order_acquire);
        if (before == values->commit && values->payload_inverse == ~values->payload &&
            values->commit == swap_test_commit(
                                  values->page_index,
                                  (unsigned int)values->writer_id,
                                  (unsigned int)values->generation,
                                  values->payload)) {
            return 0;
        }
    }

    return -1;
}

static inline void swap_test_require_lock_free_atomics(const char *test_name)
{
    _Atomic uint64_t value = 0;

    if (!atomic_is_lock_free(&value)) {
        fprintf(stderr, "%s: 64-bit atomics are not lock-free\n", test_name);
        exit(1);
    }
}

static inline int swap_test_validate_values(
    const char *test_name,
    const struct swap_record_values *values,
    size_t page,
    unsigned int writer,
    unsigned int expected_generation)
{
    uint64_t payload = swap_test_payload(page, writer, expected_generation);
    uint64_t commit = swap_test_commit(page, writer, expected_generation, payload);

    if (values->page_index == page && values->writer_id == writer && values->generation == expected_generation &&
        values->payload == payload && values->payload_inverse == ~payload && values->commit == commit) {
        return 0;
    }

    fprintf(
        stderr,
        "%s: record mismatch page=%zu writer=%u generation=%u got={page=%" PRIu64 ",writer=%" PRIu64
        ",generation=%" PRIu64 ",payload=%016" PRIx64 ",inverse=%016" PRIx64 ",commit=%016" PRIx64 "}\n",
        test_name,
        page,
        writer,
        expected_generation,
        values->page_index,
        values->writer_id,
        values->generation,
        values->payload,
        values->payload_inverse,
        values->commit);
    return -1;
}

static inline int swap_test_verify_record(
    const char *test_name,
    uint8_t *base,
    size_t page,
    size_t page_size,
    unsigned int writer,
    unsigned int generation)
{
    struct swap_record_values values;

    if (swap_test_load_record(swap_test_record(base, page, page_size, writer), &values) != 0) {
        fprintf(stderr, "%s: unstable record page=%zu writer=%u\n", test_name, page, writer);
        return -1;
    }
    return swap_test_validate_values(test_name, &values, page, writer, generation);
}

static inline void swap_test_initialize_mapping(uint8_t *base, size_t page_count, size_t page_size)
{
    for (size_t page = 0; page < page_count; page++) {
        swap_test_store_record(swap_test_record(base, page, page_size, 0), page, 0, 0);
        swap_test_store_record(swap_test_record(base, page, page_size, 1), page, 1, 0);
        atomic_store_explicit(swap_test_tail(base, page, page_size), swap_test_tail_canary(page), memory_order_release);
    }
}

static inline int swap_test_verify_mapping(
    const char *test_name,
    uint8_t *base,
    size_t page_count,
    size_t page_size,
    unsigned int generation,
    int reverse)
{
    for (size_t step = 0; step < page_count; step++) {
        size_t page = reverse ? page_count - step - 1 : step;
        uint64_t tail = atomic_load_explicit(swap_test_tail(base, page, page_size), memory_order_acquire);

        if (swap_test_verify_record(test_name, base, page, page_size, 0, generation) != 0 ||
            swap_test_verify_record(test_name, base, page, page_size, 1, generation) != 0) {
            return -1;
        }
        if (tail != swap_test_tail_canary(page)) {
            fprintf(
                stderr,
                "%s: tail mismatch page=%zu expected=%016" PRIx64 " actual=%016" PRIx64 "\n",
                test_name,
                page,
                swap_test_tail_canary(page),
                tail);
            return -1;
        }
    }

    return 0;
}

static inline void swap_test_touch_pressure(
    volatile uint8_t *base,
    size_t page_count,
    size_t page_size,
    unsigned int generation,
    const char *test_name)
{
    for (size_t page = 0; page < page_count; page++) {
        base[page * page_size] = (uint8_t)(page ^ (generation * 0x5bU));
        if ((page + 1) % SWAP_TEST_PROGRESS_PAGES == 0 || page + 1 == page_count) {
            printf("%s: pressure generation %u: %zu/%zu pages\n", test_name, generation, page + 1, page_count);
            fflush(stdout);
        }
    }
}

#endif
