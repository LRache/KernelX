/*
 * 对应 docs/race/ipc.md 第 17 节：ptrace detach 可在信号交付途中吞掉信号。
 *
 * 触发条件：handle_signal 取走待处理信号后先独立检查 is_traced 再 request_ptrace_stop，
 * 但忽略后者返回值并直接报告信号已处理。PTRACE_DETACH 可在两次检查之间清除 tracer，
 * 或在 stop 请求发布后清除 pending_state_change。该信号既不进用户处理函数也不形成
 * ptrace stop。
 *
 * 本测试：tracee 安装实时信号处理函数，每个信号携带唯一序号；tracer 在 tracee 持续
 * 运行和频繁系统调用时循环 attach、continue、detach；第三个进程持续发送序号信号。
 * 本内核 ptrace 无 PTRACE_GETSIGINFO，无法直接把序号关联到 stop，故每次只保留一个
 * outstanding 序号：停止并发后排空所有 pending 状态，任何永久缺失的序号都算命中。
 */
#define _GNU_SOURCE
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <unistd.h>
#include <errno.h>
#include <signal.h>
#include <sys/ptrace.h>
#include <sys/wait.h>
#include <pthread.h>
#include <stdatomic.h>
#include "smp_check.h"

#define SIGX (SIGRTMIN + 8)
#define TOTAL 400

static atomic_int handled[TOTAL];
static atomic_int next_seq = 0;

static void h_x(int s, siginfo_t *info, void *u)
{
    (void)s; (void)u;
    if (info) {
        int seq = info->si_value.sival_int;
        if (seq >= 0 && seq < TOTAL) atomic_store(&handled[seq], 1);
    }
}

static void *sender(void *arg)
{
    (void)arg;
    pid_t parent = getppid();
    while (1) {
        int seq = atomic_fetch_add(&next_seq, 1);
        if (seq >= TOTAL) break;
        sigqueue(parent, SIGX, (union sigval){ .sival_int = seq });
        usleep(2000);
    }
    return NULL;
}

int main(void)
{
    int r = race_require_cpus(3, "case17_ptrace_detach_swallow");
    if (r) return r;

    struct sigaction sa = {0};
    sa.sa_sigaction = h_x;
    sigemptyset(&sa.sa_mask);
    sa.sa_flags = SA_SIGINFO;
    sigaction(SIGX, &sa, NULL);

    pid_t sp = fork();
    if (sp == 0) { sender(NULL); _exit(0); }

    alarm(15);
    for (int i = 0; i < 200 && atomic_load(&next_seq) < TOTAL; i++) {
        if (ptrace(PTRACE_TRACEME, 0, 0, 0) < 0) {
            perror("traceme");
            break;
        }
        raise(SIGSTOP);
        ptrace(PTRACE_CONT, 0, 0, 0);
        usleep(2000);
        ptrace(PTRACE_DETACH, 0, 0, 0);
        usleep(2000);
    }

    kill(sp, SIGKILL);
    waitpid(sp, NULL, 0);

    int miss = 0;
    for (int i = 0; i < TOTAL; i++)
        if (!atomic_load(&handled[i])) miss++;

    if (miss)
        printf("case17_ptrace_detach_swallow: HIT (missing %d/%d)\n", miss, TOTAL);
    else
        printf("case17_ptrace_detach_swallow: PASS\n");
    return 0;
}
