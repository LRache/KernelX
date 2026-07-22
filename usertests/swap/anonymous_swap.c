#define _GNU_SOURCE

/* Anonymous swap pressure and dirty writeback regression test. */

#include <errno.h>
#include <inttypes.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/mman.h>
#include <unistd.h>

enum {
    DEFAULT_PRESSURE_MIB = 128,
    FALLBACK_MEMORY_MIB = 1024,
    MARKER_COUNT = 3,
    PROGRESS_PAGES = 16384,
};

static size_t parse_region_mib(const char *arg)
{
    char *end = NULL;
    unsigned long long mib;

    errno = 0;
    mib = strtoull(arg, &end, 0);
    if (errno != 0 || end == arg || *end != '\0' || mib == 0 || mib > SIZE_MAX / (1024UL * 1024UL)) {
        fprintf(stderr, "invalid region size in MiB: %s\n", arg);
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
    return UINT64_C(0x6a09e667f3bcc909) ^ ((uint64_t)page * UINT64_C(0x9e3779b97f4a7c15)) ^
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
            printf("  wrote generation %u: %zu/%zu pages\n", generation, page + 1, page_count);
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
                    "anonymous_swap: mismatch page=%zu marker=%u generation=%u expected=%016" PRIx64
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
                "  verified generation %u (%s): %zu/%zu pages\n",
                generation,
                reverse ? "reverse" : "forward",
                step + 1,
                page_count);
            fflush(stdout);
        }
    }

    return 0;
}

int main(int argc, char **argv)
{
    long page_size_value = sysconf(_SC_PAGESIZE);
    size_t memory_size = read_mem_total();
    size_t pressure_size = DEFAULT_PRESSURE_MIB * 1024UL * 1024UL;
    size_t region_size;
    size_t page_size;
    size_t page_count;
    uint8_t *mapping;

    if (page_size_value <= 0) {
        perror("sysconf(_SC_PAGESIZE)");
        return 1;
    }
    page_size = (size_t)page_size_value;
    if (page_size < sizeof(uint64_t) || page_size % sizeof(uint64_t) != 0) {
        fprintf(stderr, "anonymous_swap: unsupported page size %zu\n", page_size);
        return 1;
    }

    if (argc > 2) {
        fprintf(stderr, "usage: %s [region_mib]\n", argv[0]);
        return 2;
    }
    if (argc == 2) {
        region_size = parse_region_mib(argv[1]);
    } else {
        if (memory_size == 0) {
            memory_size = FALLBACK_MEMORY_MIB * 1024UL * 1024UL;
        }
        if (memory_size > SIZE_MAX - pressure_size) {
            fprintf(stderr, "anonymous_swap: region size overflow\n");
            return 1;
        }
        region_size = memory_size + pressure_size;
    }

    region_size -= region_size % page_size;
    page_count = region_size / page_size;
    printf(
        "anonymous_swap: MemTotal=%zu MiB region=%zu MiB pages=%zu page_size=%zu\n",
        memory_size / (1024UL * 1024UL),
        region_size / (1024UL * 1024UL),
        page_count,
        page_size);

    mapping = mmap(NULL, region_size, PROT_READ | PROT_WRITE, MAP_PRIVATE | MAP_ANONYMOUS, -1, 0);
    if (mapping == MAP_FAILED) {
        fprintf(stderr, "anonymous_swap: mmap failed: %s\n", strerror(errno));
        return 1;
    }

    write_mapping(mapping, page_count, page_size, 1);
    if (verify_mapping(mapping, page_count, page_size, 1, 0) != 0) {
        munmap(mapping, region_size);
        return 1;
    }

    write_mapping(mapping, page_count, page_size, 2);
    if (verify_mapping(mapping, page_count, page_size, 2, 1) != 0) {
        munmap(mapping, region_size);
        return 1;
    }

    if (munmap(mapping, region_size) != 0) {
        fprintf(stderr, "anonymous_swap: munmap failed: %s\n", strerror(errno));
        return 1;
    }

    puts("anonymous_swap: PASS");
    return 0;
}
