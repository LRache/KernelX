#include <errno.h>
#include <limits.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/types.h>
#include <sys/wait.h>
#include <unistd.h>

enum {
    DEFAULT_ROUNDS = 1024,
    DEFAULT_CHILDREN = 16,
    CHILD_EXEC_ERROR_STATUS = 127,
    STATUS_BASE = 10,
    STATUS_SPAN = 64,
};

static const char *CHILD_PATH = "fork-exec-exit-child";

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

static int expected_status(unsigned long round, unsigned long child)
{
    return STATUS_BASE + (int)((round * 17UL + child * 29UL) % STATUS_SPAN);
}

static void exec_child(unsigned long round, unsigned long child)
{
    char round_arg[32];
    char child_arg[32];
    char status_arg[32];
    char *args[] = {(char *)CHILD_PATH, round_arg, child_arg, status_arg, NULL};
    char *envp[] = {"FORK_EXEC_EXIT_STRESS=1", NULL};

    snprintf(round_arg, sizeof(round_arg), "%lu", round);
    snprintf(child_arg, sizeof(child_arg), "%lu", child);
    snprintf(status_arg, sizeof(status_arg), "%d", expected_status(round, child));

    execve(CHILD_PATH, args, envp);
    fprintf(stderr, "execve(%s) failed: %s\n", CHILD_PATH, strerror(errno));
    _exit(CHILD_EXEC_ERROR_STATUS);
}

static int spawn_child(unsigned long round, unsigned long child, pid_t *pid_out)
{
    pid_t pid;

    fflush(NULL);
    pid = fork();
    if (pid < 0) {
        fprintf(stderr, "fork failed: %s\n", strerror(errno));
        return 10;
    }
    if (pid == 0) {
        exec_child(round, child);
    }

    *pid_out = pid;
    return 0;
}

static int wait_child(pid_t pid, unsigned long round, unsigned long child)
{
    int status;
    pid_t waited;

    do {
        waited = waitpid(pid, &status, 0);
    } while (waited < 0 && errno == EINTR);

    if (waited != pid) {
        fprintf(stderr, "waitpid(%d) failed: %s\n", (int)pid, strerror(errno));
        return 20;
    }
    if (!WIFEXITED(status)) {
        if (WIFSIGNALED(status)) {
            fprintf(stderr, "child %d killed by signal %d\n", (int)pid, WTERMSIG(status));
        } else {
            fprintf(stderr, "child %d exited abnormally: status=%d\n", (int)pid, status);
        }
        return 21;
    }
    if (WEXITSTATUS(status) != expected_status(round, child)) {
        fprintf(
            stderr,
            "child %d round=%lu child=%lu exited with %d, expected %d\n",
            (int)pid,
            round,
            child,
            WEXITSTATUS(status),
            expected_status(round, child));
        return 22;
    }

    return 0;
}

static int wait_children(pid_t *pids, unsigned int count, unsigned long round)
{
    int first_ret = 0;

    for (unsigned int child = 0; child < count; child++) {
        if (pids[child] <= 0) {
            continue;
        }

        int ret = wait_child(pids[child], round, child);
        if (ret != 0 && first_ret == 0) {
            first_ret = ret;
        }
    }

    return first_ret;
}

int main(int argc, char **argv)
{
    unsigned long rounds = parse_arg(argc, argv, 1, DEFAULT_ROUNDS);
    unsigned long children = parse_arg(argc, argv, 2, DEFAULT_CHILDREN);
    pid_t *pids;

    if (children > UINT_MAX || children > SIZE_MAX / sizeof(*pids)) {
        fprintf(stderr, "children is too large\n");
        return 2;
    }

    pids = calloc(children, sizeof(*pids));
    if (pids == NULL) {
        perror("calloc");
        return 1;
    }

    printf("fork-exec-exit: rounds=%lu children=%lu\n", rounds, children);

    for (unsigned long round = 0; round < rounds; round++) {
        unsigned int started = 0;

        for (; started < (unsigned int)children; started++) {
            pids[started] = -1;

            int ret = spawn_child(round, started, &pids[started]);
            if (ret != 0) {
                wait_children(pids, started + 1, round);
                free(pids);
                return ret;
            }
        }

        int ret = wait_children(pids, started, round);
        if (ret != 0) {
            free(pids);
            return ret;
        }

        printf("completed round %lu/%lu\n", round + 1, rounds);
    }

    free(pids);
    puts("fork-exec-exit: PASS");
    return 0;
}
