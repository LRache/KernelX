#define _GNU_SOURCE

/* Preserve MAP_SHARED PTE-only dirty state across repeated fsync calls. */

#include "swap_test_common.h"

#include <fcntl.h>
#include <sys/stat.h>

enum {
    DEFAULT_PAGE_COUNT = 4,
    FIRST_GENERATION = 1,
    SECOND_GENERATION = 2,
};

static const char *const test_name = "fsync_mmap_dirty_race";

static void store_generation(uint8_t *mapping, size_t page_count, size_t page_size, unsigned int generation)
{
    for (size_t page = 0; page < page_count; page++) {
        swap_test_store_record(swap_test_record(mapping, page, page_size, 0), page, 0, generation);
        swap_test_store_record(swap_test_record(mapping, page, page_size, 1), page, 1, generation);
    }
}

static int verify_disk(int fd, uint8_t *buffer, size_t target_size, size_t page_size, unsigned int generation)
{
    size_t completed = 0;

    while (completed < target_size) {
        ssize_t length = pread(fd, buffer + completed, target_size - completed, (off_t)completed);

        if (length <= 0) {
            fprintf(
                stderr,
                "%s: direct pread failed at offset=%zu: %s\n",
                test_name,
                completed,
                length < 0 ? strerror(errno) : "unexpected EOF");
            return -1;
        }
        completed += (size_t)length;
    }

    return swap_test_verify_mapping(test_name, buffer, target_size / page_size, page_size, generation, 0);
}

int main(int argc, char **argv)
{
    const char *path = "/fsync_mmap_dirty_race.data";
    size_t target_size;
    size_t page_size = swap_test_page_size(test_name);
    size_t page_count;
    uint8_t *initial = NULL;
    uint8_t *direct_buffer = NULL;
    uint8_t *mapping = MAP_FAILED;
    int fd = -1;
    int direct_fd = -1;
    int result = 1;

    if (argc > 2) {
        fprintf(stderr, "usage: %s [target_mib]\n", argv[0]);
        return 2;
    }
    if (argc == 2) {
        target_size = swap_test_parse_mib(argv[1], "target size");
    } else {
        target_size = DEFAULT_PAGE_COUNT * page_size;
    }
    target_size = target_size / page_size * page_size;
    page_count = target_size / page_size;
    swap_test_require_lock_free_atomics(test_name);

    if (posix_memalign((void **)&initial, page_size, target_size) != 0 ||
        posix_memalign((void **)&direct_buffer, page_size, target_size) != 0) {
        fprintf(stderr, "%s: aligned allocation failed\n", test_name);
        goto cleanup;
    }
    swap_test_initialize_mapping(initial, page_count, page_size);

    fd = open(path, O_CREAT | O_TRUNC | O_RDWR, 0600);
    if (fd < 0 || pwrite(fd, initial, target_size, 0) != (ssize_t)target_size) {
        fprintf(stderr, "%s: file setup failed: %s\n", test_name, strerror(errno));
        goto cleanup;
    }

    mapping = mmap(NULL, target_size, PROT_READ | PROT_WRITE, MAP_SHARED, fd, 0);
    if (mapping == MAP_FAILED) {
        fprintf(stderr, "%s: mmap failed: %s\n", test_name, strerror(errno));
        goto cleanup;
    }
    direct_fd = open(path, O_RDONLY | O_DIRECT);
    if (direct_fd < 0) {
        fprintf(stderr, "%s: O_DIRECT open failed: %s\n", test_name, strerror(errno));
        goto cleanup;
    }

    /*
     * The initial pwrite leaves software-dirty pages behind. The first mmap
     * generation therefore reaches disk even if fsync ignores PTE dirty bits.
     */
    store_generation(mapping, page_count, page_size, FIRST_GENERATION);
    if (fsync(fd) != 0 || verify_disk(direct_fd, direct_buffer, target_size, page_size, FIRST_GENERATION) != 0) {
        fprintf(stderr, "%s: first fsync did not persist mmap data\n", test_name);
        goto cleanup;
    }

    /*
     * The first fsync cleared the software dirty state. This generation is
     * visible only through a newly set PTE D bit and catches the regression.
     */
    store_generation(mapping, page_count, page_size, SECOND_GENERATION);
    if (fsync(fd) != 0 || verify_disk(direct_fd, direct_buffer, target_size, page_size, SECOND_GENERATION) != 0) {
        fprintf(stderr, "%s: repeated fsync lost PTE-only mmap dirty data\n", test_name);
        goto cleanup;
    }

    puts("fsync_mmap_dirty_race: PASS");
    result = 0;

cleanup:
    if (direct_fd >= 0) {
        close(direct_fd);
    }
    if (mapping != MAP_FAILED) {
        munmap(mapping, target_size);
    }
    if (fd >= 0) {
        close(fd);
    }
    free(direct_buffer);
    free(initial);
    unlink(path);
    return result;
}
