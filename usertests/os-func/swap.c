#define _GNU_SOURCE
#include <stdio.h>
#include <stdlib.h>
#include <stdint.h>
#include <unistd.h>
#include <sys/mman.h>
#include <errno.h>
#include <string.h>

#define PGSIZE 4096 // 4KB
#define REGION_SIZE (512UL * 1024 * 1024)  // 512MB

int main(void) {
    // mmap 512MB
    void *base = mmap(NULL, REGION_SIZE,
                      PROT_READ | PROT_WRITE,
                      MAP_PRIVATE | MAP_ANONYMOUS,
                      -1, 0);
    if (base == MAP_FAILED) {
        fprintf(stderr, "mmap failed: %s\n", strerror(errno));
        return 1;
    }

    size_t num_pages = REGION_SIZE / PGSIZE;
    printf("Region size: %zu bytes, pages: %zu, PGSIZE=%d\n",
           (size_t)REGION_SIZE, num_pages, PGSIZE);
    
    // offset_in_bytes = idx * sizeof(uint64_t)
    // const size_t POSITIONS[] = {0, 128, 256, 384};
    const size_t POSITIONS[] = {0};
    const size_t NUM_POS = sizeof(POSITIONS) / sizeof(POSITIONS[0]);

    printf("Initializing pages (write several uint64_t per page)...\n");
    
    for (size_t page = 0; page < num_pages; ++page) {
        uint8_t *page_base = (uint8_t *)base + page * PGSIZE;

        for (size_t k = 0; k < NUM_POS; ++k) {
            size_t idx = POSITIONS[k];
            size_t byte_off = idx * sizeof(uint64_t);

            // if (byte_off + sizeof(uint64_t) > PGSIZE) {
            //     fprintf(stderr, "position %zu out of page range\n", idx);
            //     munmap(base, REGION_SIZE);
            //     return 1;
            // }

            uint64_t *ptr = (uint64_t *)(page_base + byte_off);

            uint64_t value = ((uint64_t)page << 32) ^ (uint64_t)idx;
            *ptr = value;
        }

        if (page % 1024 == 0) {
            printf("  Initialized %zu / %zu pages...\n", page, num_pages);
            fflush(stdout);
        }
    }

    for (int pass = 0; pass < 3; ++pass) {
        printf("Read/verify pass %d...\n", pass + 1);
        fflush(stdout);

        for (size_t page = 0; page < num_pages; ++page) {
            uint8_t *page_base = (uint8_t *)base + page * PGSIZE;

            for (size_t k = 0; k < NUM_POS; ++k) {
                size_t idx = POSITIONS[k];
                size_t byte_off = idx * sizeof(uint64_t);

                uint64_t *ptr = (uint64_t *)(page_base + byte_off);
                uint64_t expected = ((uint64_t)page << 32) ^ (uint64_t)idx;
                uint64_t got = *ptr;

                if (got != expected) {
                    printf("ERROR: pass %d page %zu pos_idx %zu "
                           "read=0x%016lx expected=0x%016lx\n",
                           pass + 1, page, idx,
                           (unsigned long)got, (unsigned long)expected);
                    munmap(base, REGION_SIZE);
                    return 1;
                }
            }

            if (page % 1024 == 0) {
                printf("  Verified %zu / %zu pages...\n", page, num_pages);
                fflush(stdout);
            }
        }

        printf("  Pass %d OK\n", pass + 1);
    }

    munmap(base, REGION_SIZE);
    printf("Swap test executed.\n");
    return 0;
}
