#include <errno.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <unistd.h>

enum {
    STANDALONE_STATUS = 0,
    ARG_ERROR_STATUS = 101,
    ENV_ERROR_STATUS = 102,
    STATUS_BASE = 10,
    STATUS_SPAN = 64,
};

static unsigned long parse_ulong(const char *text)
{
    char *end = NULL;
    unsigned long value;

    errno = 0;
    value = strtoul(text, &end, 0);
    if (errno != 0 || end == text || *end != '\0') {
        _exit(ARG_ERROR_STATUS);
    }

    return value;
}

static int expected_status(unsigned long round, unsigned long child)
{
    return STATUS_BASE + (int)((round * 17UL + child * 29UL) % STATUS_SPAN);
}

static int has_stress_env(char **envp)
{
    for (char **entry = envp; entry != NULL && *entry != NULL; entry++) {
        if (strcmp(*entry, "FORK_EXEC_EXIT_STRESS=1") == 0) {
            return 1;
        }
    }

    return 0;
}

int main(int argc, char **argv, char **envp)
{
    unsigned long round;
    unsigned long child;
    unsigned long expected;

    if (argc == 1) {
        return STANDALONE_STATUS;
    }

    if (argc != 4) {
        fprintf(stderr, "fork-exec-exit-child: bad argc %d\n", argc);
        _exit(ARG_ERROR_STATUS);
    }
    if (!has_stress_env(envp)) {
        fprintf(stderr, "fork-exec-exit-child: missing stress env\n");
        _exit(ENV_ERROR_STATUS);
    }

    round = parse_ulong(argv[1]);
    child = parse_ulong(argv[2]);
    expected = parse_ulong(argv[3]);
    if (expected > 255UL || expected_status(round, child) != (int)expected) {
        fprintf(stderr, "fork-exec-exit-child: bad expected status %lu\n", expected);
        _exit(ARG_ERROR_STATUS);
    }

    _exit((int)expected);
}
