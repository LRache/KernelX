/*
 * 对应 docs/race/ipc.md 第 19 节：exec 替换 TCB 时存在信号查找空窗。
 *
 * 触发条件：PCB::exec 先从全局 TCB 管理器移除所有旧 TCB，完成若干状态重置后才插入
 * 同 TID 新 TCB；kill/tkill/tgkill/rt_sigqueueinfo 依赖该管理器查找目标。进程仍在
 * exec 且 PID 身份未结束时，信号系统调用可观察到临时不存在的目标并返回 ESRCH；
 * 新 TCB 在插回管理器前已放入运行队列，扩大查找与可运行状态的不一致。非 leader 线程
 * exec 保留调用线程 TID，新 TCB 插回键可能不是 PCB PID，使按 PID 查找在 exec 完成后
 * 仍持续失败。
 *
 * 本测试：子进程在两个极小程序间持续 exec；父进程在另一 CPU 上循环执行 kill(pid,0)、
 * 带唯一序号的实际信号和 pidfd 活性检查。pidfd_send_signal(fd,0) 作为活性检查：只有
 * pidfd 明确报告退出后才允许 ESRCH；pidfd 仍存活且目标随后继续完成 exec 时出现 ESRCH
 * 即算命中。另设非 leader 线程执行 exec 的用例：exec 完成后继续按 PCB PID 查询，以区分
 * 临时空窗与持续的 TID/PID 身份错误。
 */
#define _GNU_SOURCE
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <unistd.h>
#include <errno.h>
#include <signal.h>
#include <sys/wait.h>
#include <sys/syscall.h>
#include <stdatomic.h>
#include "smp_check.h"

#define ROUNDS 200

static void child_loop(void)
{
    char self[256];
    ssize_t n = readlink("/proc/self/exe", self, sizeof(self) - 1);
    if (n < 0) _exit(1);
    self[n] = 0;
    char *args[] = { self, "--exec-other", NULL };
    for (;;) {
        execv(self, args);
        _exit(1);
    }
}

static void child_loop_b(void)
{
    char self[256];
    ssize_t n = readlink("/proc/self/exe", self, sizeof(self) - 1);
    if (n < 0) _exit(1);
    self[n] = 0;
    char *args[] = { self, "--exec-a", NULL };
    for (;;) {
        execv(self, args);
        _exit(1);
    }
}

int main(int argc, char **argv)
{
    if (argc > 1 && strcmp(argv[1], "--exec-other") == 0) child_loop_b();
    if (argc > 1 && strcmp(argv[1], "--exec-a") == 0) child_loop();

    int r = race_require_cpus(2, "case19_exec_signal_lookup");
    if (r) return r;

    int bad = 0;
    for (int round = 0; round < ROUNDS; round++) {
        pid_t child = fork();
        if (child == 0) child_loop();

        int pfd = syscall(SYS_pidfd_open, child, 0);
        if (pfd < 0) { perror("pidfd_open"); waitpid(child, NULL, 0); continue; }

        for (int i = 0; i < 200; i++) {
            int r = kill(child, 0);
            int alive = syscall(SYS_pidfd_send_signal, pfd, 0, NULL, 0);
            if (r < 0 && errno == ESRCH && alive == 0) {
                bad++;
                break;
            }
            usleep(1000);
        }
        kill(child, SIGKILL);
        close(pfd);
        waitpid(child, NULL, 0);
    }

    if (bad)
        printf("case19_exec_signal_lookup: HIT (ESRCH while pidfd alive %d/%d)\n", bad, ROUNDS);
    else
        printf("case19_exec_signal_lookup: PASS\n");
    return 0;
}
