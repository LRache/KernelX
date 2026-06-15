#define _GNU_SOURCE

#include <errno.h>
#include <limits.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/mman.h>
#include <sys/syscall.h>
#include <sys/types.h>
#include <sys/wait.h>
#include <unistd.h>

enum {
    DEFAULT_ROUNDS = 512,
    DEFAULT_CHILDREN = 8,
    DEFAULT_MAP_PAGES = 8,
    DEFAULT_EXIT_PAGES = 4,
};

struct test_config {
    unsigned long rounds;
    unsigned int children;
    size_t map_len;
    size_t exit_len;
    size_t page_size;
};

static unsigned long parse_arg(int argc, char **argv, int index, unsigned long fallback)
{
    char *end = NULL;
    unsigned long value;

    if (argc <= index) {
        return fallback;
    }

    errno = 0;
    value = strtoul(argv[index], &end, 0);
    if (errno != 0 || end == argv[index] || *end != '\0' || value == 0) {
        fprintf(stderr, "bad numeric argument %d: %s\n", index, argv[index]);
        exit(2);
    }

    return value;
}

static void *raw_mmap(size_t len)
{
    long ret = syscall(
        SYS_mmap,
        NULL,
        len,
        PROT_READ | PROT_WRITE,
        MAP_PRIVATE | MAP_ANONYMOUS,
        -1,
        0);

    if (ret == -1) {
        return MAP_FAILED;
    }

    return (void *)ret;
}

static int raw_munmap(void *addr, size_t len)
{
    long ret = syscall(SYS_munmap, addr, len);

    if (ret == -1) {
        return -1;
    }

    return (int)ret;
}

static void raw_exit(int status)
{
    syscall(SYS_exit, status);
    __builtin_unreachable();
}

static unsigned char pattern_seed(unsigned long round, unsigned int child)
{
    return (unsigned char)(0x31UL + round * 17UL + child * 29UL);
}

static void fill_mapping(void *addr, size_t len, size_t page_size, unsigned char seed)
{
    volatile unsigned char *p = (volatile unsigned char *)addr;

    for (size_t off = 0; off < len; off += page_size) {
        p[off] = (unsigned char)(seed + off / page_size);
    }
    p[len - 1] = (unsigned char)(seed ^ 0xa5U);
}

static int check_mapping(void *addr, size_t len, size_t page_size, unsigned char seed)
{
    volatile unsigned char *p = (volatile unsigned char *)addr;

    for (size_t off = 0; off < len; off += page_size) {
        if (p[off] != (unsigned char)(seed + off / page_size)) {
            return -1;
        }
    }

    if (p[len - 1] != (unsigned char)(seed ^ 0xa5U)) {
        return -1;
    }

    return 0;
}

static void child_work(void *inherited, const struct test_config *cfg, unsigned long round, unsigned int child)
{
    unsigned char seed = pattern_seed(round, child);
    void *area;
    void *exit_area;

    if (check_mapping(inherited, cfg->map_len, cfg->page_size, seed) != 0) {
        raw_exit(101);
    }

    fill_mapping(inherited, cfg->map_len, cfg->page_size, (unsigned char)(seed ^ 0x5aU));
    if (raw_munmap(inherited, cfg->map_len) != 0) {
        raw_exit(102);
    }

    area = raw_mmap(cfg->map_len);
    if (area == MAP_FAILED) {
        raw_exit(103);
    }

    fill_mapping(area, cfg->map_len, cfg->page_size, (unsigned char)(seed + 1U));
    if (raw_munmap(area, cfg->map_len) != 0) {
        raw_exit(104);
    }

    exit_area = raw_mmap(cfg->exit_len);
    if (exit_area == MAP_FAILED) {
        raw_exit(105);
    }

    fill_mapping(exit_area, cfg->exit_len, cfg->page_size, (unsigned char)(seed + 2U));
    raw_exit(0);
}

static int spawn_child(const struct test_config *cfg, unsigned long round, unsigned int child, pid_t *pid_out)
{
    unsigned char seed = pattern_seed(round, child);
    void *inherited = raw_mmap(cfg->map_len);
    pid_t pid;

    if (inherited == MAP_FAILED) {
        fprintf(stderr, "parent mmap failed: %s\n", strerror(errno));
        return 10;
    }

    fill_mapping(inherited, cfg->map_len, cfg->page_size, seed);
    fflush(NULL);

    pid = fork();
    if (pid < 0) {
        int saved_errno = errno;

        raw_munmap(inherited, cfg->map_len);
        fprintf(stderr, "fork failed: %s\n", strerror(saved_errno));
        return 11;
    }

    if (pid == 0) {
        child_work(inherited, cfg, round, child);
    }

    *pid_out = pid;
    if (raw_munmap(inherited, cfg->map_len) != 0) {
        fprintf(stderr, "parent munmap failed: %s\n", strerror(errno));
        return 12;
    }

    return 0;
}

static int wait_children(pid_t *pids, unsigned int count)
{
    for (unsigned int i = 0; i < count; i++) {
        int status;
        pid_t waited;

        if (pids[i] <= 0) {
            continue;
        }

        do {
            waited = waitpid(pids[i], &status, 0);
        } while (waited < 0 && errno == EINTR);

        if (waited != pids[i]) {
            fprintf(stderr, "waitpid(%d) failed: %s\n", pids[i], strerror(errno));
            return 20;
        }

        if (!WIFEXITED(status) || WEXITSTATUS(status) != 0) {
            if (WIFSIGNALED(status)) {
                fprintf(stderr, "child %d killed by signal %d\n", pids[i], WTERMSIG(status));
            } else {
                fprintf(stderr, "child %d exited with status %d\n", pids[i], status);
            }
            return 21;
        }
    }

    return 0;
}

static size_t pages_to_len(unsigned long pages, size_t page_size, const char *name)
{
    if (pages > SIZE_MAX / page_size) {
        fprintf(stderr, "%s is too large\n", name);
        exit(2);
    }

    return pages * page_size;
}

int main(int argc, char **argv)
{
    unsigned long rounds = parse_arg(argc, argv, 1, DEFAULT_ROUNDS);
    unsigned long children = parse_arg(argc, argv, 2, DEFAULT_CHILDREN);
    unsigned long map_pages = parse_arg(argc, argv, 3, DEFAULT_MAP_PAGES);
    unsigned long exit_pages = parse_arg(argc, argv, 4, DEFAULT_EXIT_PAGES);
    long page_size = sysconf(_SC_PAGESIZE);
    struct test_config cfg;
    pid_t *pids;

    if (page_size <= 0) {
        perror("sysconf(_SC_PAGESIZE)");
        return 1;
    }
    if (children > UINT_MAX) {
        fprintf(stderr, "children is too large\n");
        return 2;
    }

    cfg.rounds = rounds;
    cfg.children = (unsigned int)children;
    cfg.page_size = (size_t)page_size;
    cfg.map_len = pages_to_len(map_pages, cfg.page_size, "map_pages");
    cfg.exit_len = pages_to_len(exit_pages, cfg.page_size, "exit_pages");

    pids = calloc(cfg.children, sizeof(*pids));
    if (pids == NULL) {
        perror("calloc");
        return 1;
    }

    printf(
        "fork-mmap-munmap-exit: rounds=%lu children=%u map_pages=%lu exit_pages=%lu\n",
        cfg.rounds,
        cfg.children,
        map_pages,
        exit_pages);

    for (unsigned long round = 0; round < cfg.rounds; round++) {
        unsigned int started = 0;

        for (; started < cfg.children; started++) {
            pids[started] = -1;

            int ret = spawn_child(&cfg, round, started, &pids[started]);
            if (ret != 0) {
                wait_children(pids, started + 1);
                free(pids);
                return ret;
            }
        }

        int ret = wait_children(pids, started);
        if (ret != 0) {
            free(pids);
            return ret;
        }

        if ((round + 1) % 64 == 0) {
            printf("completed round %lu/%lu\n", round + 1, cfg.rounds);
        }
    }

    free(pids);
    puts("fork-mmap-munmap-exit: PASS");
    return 0;
}
