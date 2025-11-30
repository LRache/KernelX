#include <sys/types.h>
#define _GNU_SOURCE
#include <stdio.h>
#include <stdlib.h>
#include <stdint.h>
#include <unistd.h>
#include <sys/mman.h>
#include <errno.h>
#include <string.h>
#include <sys/wait.h>

#define PGSIZE 4096 // 4KB
#define REGION_SIZE (512UL * 1024 * 1024)  // 512MB

static const size_t POSITIONS[] = {0};
static const size_t NUM_POSITIONS = sizeof(POSITIONS) / sizeof(POSITIONS[0]);

int verify_page(uint8_t *page_base, size_t page_num) {
    for (size_t k = 0; k < NUM_POSITIONS; ++k) {
        size_t idx = POSITIONS[k];
        size_t byte_off = idx * sizeof(uint64_t);
        uint64_t *ptr = (uint64_t *)(page_base + byte_off);
        uint64_t expected = ((uint64_t)page_num << 32) ^ (uint64_t)idx;
        uint64_t got = *ptr;

        if (got != expected) {
            fprintf(stderr, "    MISMATCH at page %zu(%p), position %zu: expected 0x%016lx, got 0x%016lx\n",
                   page_num, ptr, idx, expected, got);
            fflush(stderr);
            return 1; // Mismatch
        }
    }
    return 0; // All positions match
}

int verify_pages(uint8_t *base, size_t num_pages) {
    printf("Verifying pages...\n");
    fflush(stdout);
    
    for (size_t page = 0; page < num_pages; ++page) {
        uint8_t *page_base = base + page * PGSIZE;
        if (verify_page(page_base, page) != 0) {
            return 1; // Mismatch found
        }

        if (page % 1024 == 0) {
            printf("  Verified %zu / %zu pages...\n", page, num_pages);
            fflush(stdout);
        }
    }
    
    return 0; // All pages verified
}

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

    printf("Initializing pages (write several uint64_t per page)...\n");
    
    for (size_t page = 0; page < num_pages; ++page) {
        uint8_t *page_base = (uint8_t *)base + page * PGSIZE;

        for (size_t k = 0; k < NUM_POSITIONS; ++k) {
            size_t idx = POSITIONS[k];
            size_t byte_off = idx * sizeof(uint64_t);

            uint64_t *ptr = (uint64_t *)(page_base + byte_off);

            uint64_t value = ((uint64_t)page << 32) ^ (uint64_t)idx;
            *ptr = value;
        }

        if (page % 1024 == 0) {
            printf("  Initialized %zu / %zu pages...\n", page, num_pages);
            fflush(stdout);
        }
    }

    int pass = 0;
    for (; pass < 1; ++pass) {
        verify_pages((uint8_t *)base, num_pages);
        printf("  Pass %d OK\n", pass + 1);
        fflush(stdout);
    }

    pid_t pid = fork();
    if (pid < 0) {
        perror("fork");
        return 1;
    }

    if (pid == 0) {
        if (verify_pages((uint8_t *)base, num_pages) != 0) {
            fprintf(stderr, "Child process verification failed\n");
            return 1;
        }
        
        printf("  Child process verification OK\n");
        fflush(stdout);

        return 0;
    }

    for (; pass < 2; ++pass) {
        verify_pages((uint8_t *)base, num_pages);
        printf("  Pass %d OK\n", pass + 1);
        fflush(stdout);
    }

    wait(NULL);

    munmap(base, REGION_SIZE);
    return 0;
}
