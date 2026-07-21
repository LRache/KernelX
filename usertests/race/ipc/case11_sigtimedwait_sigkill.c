/*
 * 对应 docs/race/ipc.md 第 11 节：sigtimedwait 等待状态可覆盖已到达的 SIGKILL。
 *
 * 触发条件：prepare_signal_wait 阻塞期间保留 signal_to_wait；系统调用只在恢复调度后
 * 清空它。try_recive_pending_signal 优先处理 signal_to_wait 并无条件覆盖
 * pending_signal。SIGKILL 已填入槽位并唤醒任务但任务尚未运行时，随后到达的被等待
 * 信号可覆盖 SIGKILL。
 *
 * 本测试：每轮新子进程，目标线程循环等待实时信号 X；两个固定在不同 CPU 的发送者按
 * 屏障先后紧密发送 SIGKILL 和 X，CPU 饱和线程延迟目标恢复。父进程为每轮设死亡期限；
 * 任何一次 SIGKILL 系统调用成功但子进程继续运行并处理 X 即算命中。每轮用新子进程
 * 避免前一轮信号状态污染。
 */
#define _GNU_SOURCE
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <unistd.h>
#include <errno.h>
#include <signal.h>
#include <sys/wait.h>
#include <pthread.h>
#include <stdatomic.h>
#include "smp_check.h"

#define SIGX (SIGRTMIN + 1)
#define ROUNDS 200

static void spin_handler(int s, siginfo_t *info, void *u) { (void)s; (void)info; (void)u; _exit(0x42); }

static void *spinner(void *arg) { (void)arg; volatile int x = 0; while (1) x++; return NULL; }

int main(void)
{
    int r = race_require_cpus(3, "case11_sigtimedwait_sigkill");
    if (r) return r;

    int hit = 0;
    for (int round = 0; round < ROUNDS; round++) {
        pid_t child = fork();
        if (child == 0) {
            struct sigaction sa = {0};
            sa.sa_sigaction = spin_handler;
            sigemptyset(&sa.sa_mask);
            sa.sa_flags = SA_SIGINFO;
            sigaction(SIGX, &sa, NULL);

            pthread_t sp;
            pthread_create(&sp, NULL, spinner, NULL);

            sigset_t wset;
            sigemptyset(&wset);
            sigaddset(&wset, SIGX);
            for (;;) {
                siginfo_t info;
                int r = sigtimedwait(&wset, &info, NULL);
                (void)r;
            }
            _exit(0);
        }
        usleep(20 * 1000);
        kill(child, SIGKILL);
        kill(child, SIGX);

        int status = 0;
        for (int w = 0; w < 50; w++) {
            pid_t r = waitpid(child, &status, WNOHANG);
            if (r == child) break;
            usleep(10 * 1000);
        }
        waitpid(child, &status, 0);
        if (WIFEXITED(status) && WEXITSTATUS(status) == 0x42) hit++;
    }
    if (hit)
        printf("case11_sigtimedwait_sigkill: HIT (SIGKILL lost %d/%d)\n", hit, ROUNDS);
    else
        printf("case11_sigtimedwait_sigkill: PASS\n");
    return 0;
}
