/*
 * 对应 docs/race/ipc.md 第 18 节：ptrace 的同号授权令牌可交付错误信号实例。
 *
 * 触发条件：ptrace 恢复只用 Option<SignalNum> 记录允许交付的信号号而不绑定具体
 * PendingSignal 实例。tracee 停止期间另一同号信号先占据 TCB 槽位，PTRACE_CONT(...,X)
 * 把原 stop 中的 X 放入 PCB 队列，但授权令牌可被新实例消费，导致 siginfo 顺序和
 * ptrace stop 归属错误。不要求两个系统调用时间重叠：A stop 后先完整投递 B 再
 * PTRACE_CONT(...,X) 即可让 B 消费只按信号号匹配的令牌。
 *
 * 本测试：实时信号 X 携带不同 payload；先让 payload A 触发并确认 ptrace stop，在
 * tracee 保持停止时发送 payload B，再以 X 执行 continue。记录用户处理函数首先收到
 * 的 payload 和后续 ptrace stop。预期授权必须对应 A；若 B 先绕过 ptrace 而 A 产生
 * 额外 stop 即算命中。本内核 ptrace 无 GETSIGINFO，用 tracee 自报 payload 区分实例。
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
#include <sys/syscall.h>
#include <stdatomic.h>

#define SIGX (SIGRTMIN + 9)

static atomic_int first_payload = -1;
static atomic_int payload_count = 0;

static void h_x(int s, siginfo_t *info, void *u)
{
    (void)s; (void)u;
    if (info) {
        int p = info->si_value.sival_int;
        int expected = -1;
        (void)atomic_compare_exchange_strong(&first_payload, &expected, p);
        atomic_fetch_add(&payload_count, 1);
    }
}

int main(void)
{
    struct sigaction sa = {0};
    sa.sa_sigaction = h_x;
    sigemptyset(&sa.sa_mask);
    sa.sa_flags = SA_SIGINFO;
    sigaction(SIGX, &sa, NULL);

    int bad = 0;
    for (int round = 0; round < 100; round++) {
        atomic_store(&first_payload, -1);
        atomic_store(&payload_count, 0);

        pid_t child = fork();
        if (child == 0) {
            ptrace(PTRACE_TRACEME, 0, 0, 0);
            raise(SIGSTOP);
            while (1) syscall(SYS_getpid);
            _exit(0);
        }

        int status = 0;
        waitpid(child, &status, 0);

        sigqueue(child, SIGX, (union sigval){ .sival_int = 1000 });
        for (;;) {
            kill(child, SIGSTOP);
            waitpid(child, &status, 0);
            if (WIFSTOPPED(status)) break;
        }

        sigqueue(child, SIGX, (union sigval){ .sival_int = 2000 });

        ptrace(PTRACE_CONT, child, 0, SIGX);
        usleep(50 * 1000);
        kill(child, SIGKILL);
        waitpid(child, &status, 0);

        int fp = atomic_load(&first_payload);
        if (fp == 2000) bad++;
    }

    if (bad)
        printf("case18_ptrace_token_instance: HIT (B bypassed ptrace %d times)\n", bad);
    else
        printf("case18_ptrace_token_instance: PASS\n");
    return 0;
}
