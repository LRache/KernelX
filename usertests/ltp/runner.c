#define _GNU_SOURCE

#include <errno.h>
#include <signal.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/wait.h>
#include <unistd.h>

#define LTPROOT "/tests/ltp"
#define DEFAULT_RUNTEST LTPROOT "/kx.runtest"
#define MAX_LINE 1024
#define MAX_ARGS 64

/* LTP 测试以 TCONF 结束时的退出码，表示当前环境不支持该测试。 */
#define LTP_EXIT_TCONF 32

/*
 * 单个测试的兜底超时（秒）。LTP 新式测试自带约 30s 的超时，但它依赖内核的定时器和信号，
 * 老式测试则没有超时；这里保证一个卡住的测试不会拖住整次运行。
 */
#define DEFAULT_TIMEOUT 300
/* 超时后先发 SIGINT 让 LTP 库清理测试进程组，等待这么多秒后再 SIGKILL。 */
#define KILL_GRACE 5

static volatile sig_atomic_t alarm_fired;

static void on_alarm(int sig) {
    (void)sig;
    alarm_fired = 1;
}

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

/* 等待测试进程结束；超时返回 1，并保证进程已被回收。 */
static int wait_with_timeout(pid_t pid, int *status, unsigned int timeout) {
    int signals[] = {SIGINT, SIGKILL};
    unsigned int delays[] = {timeout, KILL_GRACE};

    for (int i = 0; i < 2; i++) {
        alarm_fired = 0;
        alarm(delays[i]);
        pid_t ret = waitpid(pid, status, 0);
        alarm(0);
        if (ret == pid) {
            return i > 0;
        }
        if (errno != EINTR || !alarm_fired) {
            perror("waitpid");
            kill(-pid, SIGKILL);
            break;
        }
        kill(-pid, signals[i]);
    }

    while (waitpid(pid, status, 0) < 0 && errno == EINTR) {
    }
    return 1;
}

int main(int argc, char *argv[]) {
    const char *runtest = argc > 1 ? argv[1] : DEFAULT_RUNTEST;
    unsigned int timeout = argc > 2 ? (unsigned int)atoi(argv[2]) : DEFAULT_TIMEOUT;
    char path[MAX_LINE];

    if (timeout == 0) {
        timeout = DEFAULT_TIMEOUT;
    }

    struct sigaction alarm_action = {.sa_handler = on_alarm};
    sigemptyset(&alarm_action.sa_mask);
    /* 不设置 SA_RESTART，让 SIGALRM 打断 waitpid。 */
    sigaction(SIGALRM, &alarm_action, NULL);

    if (runtest[0] != '/') {
        snprintf(path, sizeof(path), LTPROOT "/%s", runtest);
        runtest = path;
    }

    setenv("LTPROOT", LTPROOT, 1);
    setenv("TMPDIR", "/tmp", 1);
    setenv("KCONFIG_PATH", LTPROOT "/kernel.config", 1);
    setenv("PATH", LTPROOT "/testcases/bin:" LTPROOT "/bin:/bin:/usr/bin", 1);
    if (chdir(LTPROOT "/testcases/bin") < 0) {
        perror("chdir " LTPROOT "/testcases/bin");
        return 1;
    }

    FILE *file = fopen(runtest, "r");
    if (file == NULL) {
        perror(runtest);
        return 1;
    }

    printf("[ltp] runtest %s timeout=%us\n", runtest, timeout);
    fflush(stdout);

    char line[MAX_LINE];
    int passed = 0;
    int conf = 0;
    int failed = 0;
    int skipped = 0;
    int timedout = 0;

    while (fgets(line, sizeof(line), file) != NULL) {
        char *entry = trim(line);
        if (*entry == '\0' || *entry == '#') {
            continue;
        }

        char *args[MAX_ARGS];
        int nargs = split_args(entry, args);
        if (nargs < 2) {
            continue;
        }

        const char *tag = args[0];
        char **command = &args[1];

        int needs_shell = 0;
        for (int i = 1; i < nargs; i++) {
            if (strpbrk(args[i], "|;&<>$`'\"*?") != NULL) {
                needs_shell = 1;
                break;
            }
        }
        if (needs_shell) {
            printf("[ltp] SKIP %s (needs shell)\n", tag);
            skipped++;
            continue;
        }

        printf("[ltp] run %s\n", tag);
        fflush(stdout);

        pid_t pid = fork();
        if (pid < 0) {
            perror("fork");
            failed++;
            continue;
        }

        if (pid == 0) {
            /* 独立进程组，超时时可以连同测试派生的进程一起终止。 */
            setpgid(0, 0);
            execvp(command[0], command);
            perror("execvp");
            _exit(127);
        }

        setpgid(pid, pid);

        int status = 0;
        if (wait_with_timeout(pid, &status, timeout)) {
            printf("[ltp] FAIL %s timeout=%us\n", tag, timeout);
            failed++;
            timedout++;
            fflush(stdout);
            continue;
        }

        if (WIFEXITED(status) && WEXITSTATUS(status) == 0) {
            printf("[ltp] PASS %s\n", tag);
            passed++;
        } else if (WIFEXITED(status) && WEXITSTATUS(status) == LTP_EXIT_TCONF) {
            printf("[ltp] CONF %s\n", tag);
            conf++;
        } else if (WIFEXITED(status)) {
            printf("[ltp] FAIL %s exit=%d\n", tag, WEXITSTATUS(status));
            failed++;
        } else if (WIFSIGNALED(status)) {
            printf("[ltp] FAIL %s signal=%d\n", tag, WTERMSIG(status));
            failed++;
        } else {
            printf("[ltp] FAIL %s status=0x%x\n", tag, status);
            failed++;
        }
        fflush(stdout);
    }

    fclose(file);

    printf("[ltp] summary pass=%d conf=%d fail=%d skip=%d timeout=%d\n", passed, conf, failed, skipped, timedout);
    return failed == 0 ? 0 : 1;
}
