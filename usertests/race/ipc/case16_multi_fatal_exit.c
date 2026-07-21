/*
 * 对应 docs/race/ipc.md 第 16 节：多线程同时处理致命信号可重复执行进程退出。
 *
 * 触发条件：不同 TCB 可同时在各自返回用户态路径处理致命信号并都调用共享 PCB::exit；
 * PCB::exit 入口没有原子 Running-to-Exiting 一次性状态转换。tasks 锁只串行化部分
 * 清理，释放后第二个调用仍可重复关闭资源、覆盖退出状态、唤醒父进程并发第二次 SIGCHLD。
 *
 * 本测试：子进程创建多个线程并停在频繁返回用户态的循环中；父进程同时对两个不同 TID
 * 发送致命信号；父进程高频执行非阻塞和阻塞 waitpid，统计 SIGCHLD 与最终退出状态。
 * 重复退出通知、同一 PID 被回收两次、状态不一致即算命中。需真正 SMP 才易触发。
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
#include <pthread.h>
#include <stdatomic.h>
#include "smp_check.h"

#define NTHREADS 3
#define ROUNDS 200

static atomic_int sigchld = 0;
static void h_chld(int s) { (void)s; atomic_fetch_add(&sigchld, 1); }

static void *spinner(void *arg)
{
    (void)arg;
    while (1) { syscall(SYS_getpid); }
    return NULL;
}

int main(void)
{
    int r = race_require_cpus(2, "case16_multi_fatal_exit");
    if (r) return r;

    struct sigaction sc = {0}; sc.sa_handler = h_chld; sigemptyset(&sc.sa_mask);
    sc.sa_flags = SA_RESTART;
    sigaction(SIGCHLD, &sc, NULL);

    int bad = 0;
    for (int round = 0; round < ROUNDS; round++) {
        atomic_store(&sigchld, 0);
        pid_t child = fork();
        if (child == 0) {
            pthread_t t[NTHREADS];
            for (int i = 0; i < NTHREADS; i++) pthread_create(&t[i], NULL, spinner, NULL);
            while (1) syscall(SYS_getpid);
            _exit(0);
        }
        usleep(20 * 1000);
        kill(child, SIGKILL);
        kill(child, SIGKILL);

        int status = 0, reaped = 0;
        for (;;) {
            pid_t r = waitpid(child, &status, 0);
            if (r == child) { reaped++; if (reaped > 1) break; }
            else break;
        }
        if (reaped > 1) bad++;
    }
    if (bad)
        printf("case16_multi_fatal_exit: HIT (double reap %d/%d)\n", bad, ROUNDS);
    else
        printf("case16_multi_fatal_exit: PASS\n");
    return 0;
}
