#define _GNU_SOURCE

#include <errno.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/mount.h>
#include <sys/stat.h>
#include <sys/wait.h>
#include <unistd.h>

#define TEST_LIST "/etc/kx-tests.list"
#define MAX_LINE 512
#define MAX_ARGS 32

static char *trim(char *line) {
    while (*line == ' ' || *line == '\t' || *line == '\n' || *line == '\r') {
        line++;
    }

    char *end = line + strlen(line);
    while (end > line && (end[-1] == ' ' || end[-1] == '\t' || end[-1] == '\n' || end[-1] == '\r')) {
        *--end = '\0';
    }

    return line;
}

static int prepare_tmp(void) {
    if (mkdir("/tmp", 0777) < 0 && errno != EEXIST) {
        perror("mkdir /tmp");
        return -1;
    }

    if (mount("tmpfs", "/tmp", "tmpfs", 0, "") < 0 && errno != EBUSY) {
        perror("mount /tmp");
        return -1;
    }

    return 0;
}

static int split_args(char *line, char *argv[MAX_ARGS]) {
    int argc = 0;
    char *saveptr = NULL;
    char *token = strtok_r(line, " \t", &saveptr);

    while (token != NULL && argc < MAX_ARGS - 1) {
        argv[argc++] = token;
        token = strtok_r(NULL, " \t", &saveptr);
    }

    argv[argc] = NULL;
    return argc;
}

static int run_test(char *line) {
    char *argv[MAX_ARGS];
    int argc = split_args(line, argv);
    if (argc == 0) {
        return 0;
    }

    printf("[init] run %s\n", argv[0]);
    fflush(stdout);

    pid_t pid = fork();
    if (pid < 0) {
        perror("fork");
        return 1;
    }

    if (pid == 0) {
        execv(argv[0], argv);
        perror("execv");
        _exit(127);
    }

    int status = 0;
    if (waitpid(pid, &status, 0) < 0) {
        perror("waitpid");
        return 1;
    }

    if (WIFEXITED(status) && WEXITSTATUS(status) == 0) {
        printf("[init] pass %s\n", argv[0]);
        return 0;
    }

    if (WIFEXITED(status)) {
        printf("[init] fail %s exit=%d\n", argv[0], WEXITSTATUS(status));
    } else if (WIFSIGNALED(status)) {
        printf("[init] fail %s signal=%d\n", argv[0], WTERMSIG(status));
    } else {
        printf("[init] fail %s status=0x%x\n", argv[0], status);
    }

    return 1;
}

int main(void) {
    printf("[init] KernelX usertests autorun\n");
    prepare_tmp();

    FILE *file = fopen(TEST_LIST, "r");
    if (file == NULL) {
        perror("open " TEST_LIST);
        return 1;
    }

    char line[MAX_LINE];
    int total = 0;
    int failed = 0;

    while (fgets(line, sizeof(line), file) != NULL) {
        char *command = trim(line);
        if (*command == '\0' || *command == '#') {
            continue;
        }

        total++;
        failed += run_test(command);
    }

    fclose(file);
    sync();

    printf("[init] summary total=%d failed=%d\n", total, failed);
    return failed == 0 ? 0 : 1;
}
