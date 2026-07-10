#define _GNU_SOURCE

#include <errno.h>
#include <pthread.h>
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
    DEFAULT_ROUNDS = 4096,
    DEFAULT_THREADS = 4,
    DEFAULT_MAP_PAGES = 8,
    DEFAULT_CHILD_FORKS = 2,
};

struct worker_arg {
    unsigned int round;
    unsigned int worker;
    unsigned int child_forks;
    size_t map_len;
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

static void touch_mapping(void *addr, size_t len, size_t page_size, unsigned char seed)
{
    volatile unsigned char *p = (volatile unsigned char *)addr;

    for (size_t off = 0; off < len; off += page_size) {
        p[off] = (unsigned char)(seed + off / page_size);
    }
    p[len - 1] = seed;
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

static void child_work(void *inherited, const struct worker_arg *arg, unsigned int iter)
{
    void *area;

    touch_mapping(inherited, arg->map_len, arg->page_size, (unsigned char)(arg->worker + iter));
    if (raw_munmap(inherited, arg->map_len) != 0) {
        raw_exit(101);
    }

    area = raw_mmap(arg->map_len);
    if (area == MAP_FAILED) {
        raw_exit(102);
    }

    touch_mapping(area, arg->map_len, arg->page_size, (unsigned char)(arg->round + iter));
    if (raw_munmap(area, arg->map_len) != 0) {
        raw_exit(103);
    }

    raw_exit(0);
}

static int run_child(const struct worker_arg *arg, unsigned int iter)
{
    int status;
    pid_t pid;
    void *area = raw_mmap(arg->map_len);

    if (area == MAP_FAILED) {
        fprintf(stderr, "parent mmap failed: %s\n", strerror(errno));
        return 10;
    }

    touch_mapping(area, arg->map_len, arg->page_size, (unsigned char)(arg->round + arg->worker));

    pid = fork();
    if (pid < 0) {
        int saved_errno = errno;
        raw_munmap(area, arg->map_len);
        fprintf(stderr, "fork failed: %s\n", strerror(saved_errno));
        return 11;
    }

    if (pid == 0) {
        child_work(area, arg, iter);
    }

    if (raw_munmap(area, arg->map_len) != 0) {
        fprintf(stderr, "parent munmap failed: %s\n", strerror(errno));
        return 12;
    }

    if (waitpid(pid, &status, 0) != pid) {
        fprintf(stderr, "waitpid failed: %s\n", strerror(errno));
        return 13;
    }

    if (!WIFEXITED(status) || WEXITSTATUS(status) != 0) {
        fprintf(stderr, "child exited unexpectedly: status=%d\n", status);
        return 14;
    }

    return 0;
}

static void *worker_main(void *data)
{
    const struct worker_arg *arg = data;

    for (unsigned int iter = 0; iter < arg->child_forks; iter++) {
        int ret = run_child(arg, iter);
        if (ret != 0) {
            return (void *)(intptr_t)ret;
        }
    }

    return NULL;
}

int main(int argc, char **argv)
{
    unsigned long rounds = parse_arg(argc, argv, 1, DEFAULT_ROUNDS);
    unsigned long threads = parse_arg(argc, argv, 2, DEFAULT_THREADS);
    unsigned long map_pages = parse_arg(argc, argv, 3, DEFAULT_MAP_PAGES);
    unsigned long child_forks = parse_arg(argc, argv, 4, DEFAULT_CHILD_FORKS);
    long page_size = sysconf(_SC_PAGESIZE);
    pthread_t *tids;
    struct worker_arg *args;

    if (page_size <= 0) {
        perror("sysconf(_SC_PAGESIZE)");
        return 1;
    }

    tids = calloc(threads, sizeof(*tids));
    args = calloc(threads, sizeof(*args));
    if (tids == NULL || args == NULL) {
        perror("calloc");
        free(tids);
        free(args);
        return 1;
    }

    printf(
        "thread_fork_mmap_leak: rounds=%lu threads=%lu map_pages=%lu child_forks=%lu\n",
        rounds,
        threads,
        map_pages,
        child_forks);

    for (unsigned long round = 0; round < rounds; round++) {
        for (unsigned long i = 0; i < threads; i++) {
            args[i].round = (unsigned int)round;
            args[i].worker = (unsigned int)i;
            args[i].child_forks = (unsigned int)child_forks;
            args[i].map_len = map_pages * (size_t)page_size;
            args[i].page_size = (size_t)page_size;

            int ret = pthread_create(&tids[i], NULL, worker_main, &args[i]);
            if (ret != 0) {
                fprintf(stderr, "pthread_create failed: %s\n", strerror(ret));
                free(tids);
                free(args);
                return 1;
            }
        }

        for (unsigned long i = 0; i < threads; i++) {
            void *result = NULL;
            int ret = pthread_join(tids[i], &result);
            if (ret != 0) {
                fprintf(stderr, "pthread_join failed: %s\n", strerror(ret));
                free(tids);
                free(args);
                return 1;
            }
            if (result != NULL) {
                fprintf(stderr, "worker failed: %ld\n", (long)(intptr_t)result);
                free(tids);
                free(args);
                return 1;
            }
        }

        if ((round + 1) % 64 == 0) {
            printf("completed round %lu/%lu\n", round + 1, rounds);
        }
    }

    free(tids);
    free(args);
    puts("thread_fork_mmap_leak: PASS");
    return 0;
}
