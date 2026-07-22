/*
 * 对应 docs/race/ipc.md 第 15 节：临时信号 mask 与进程待处理队列之间丢失唤醒。
 *
 * 触发条件：rt_sigsuspend 的 mask 替换、PCB 待处理队列提取和 TCB 阻塞无共同锁；
 * 发送者按旧 mask 判定信号被屏蔽却在目标完成新 mask 下队列扫描后才入队，且入队不
 * 唤醒。pselect/ppoll/epoll_pwait 临时 mask 路径存在同类窗口。
 *
 * 本测试：X 在常态 mask 中保持屏蔽，每轮通过 sigsuspend 临时解除屏蔽并进入无限等待；
 * 另一个 CPU 每轮只发送一次 X，并扫描屏障释放后的细小延迟。watchdog 记录未因 X 返回
 * 的轮次，再用无关 fd 事件唤醒目标并确认 X 已在 pending 集合中。注：本内核 pselect6
 * 第 6 参数 ABI 与 Linux 不一致（直接当 SignalSet*），故 pselect 变体用裸 syscall 不可
 * 移植，这里覆盖 sigsuspend、ppoll、epoll_pwait 三种。
 */
#define _GNU_SOURCE
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <unistd.h>
#include <errno.h>
#include <signal.h>
#include <poll.h>
#include <sys/epoll.h>
#include <pthread.h>
#include <stdatomic.h>
#include "smp_check.h"

#define SIGX (SIGRTMIN + 7)
#define ROUNDS 200

static atomic_int x_handled = 0;
static atomic_int hit = 0;
static int pipefd[2];

static void h_x(int s) { (void)s; atomic_store(&x_handled, 1); }

static void on_deadline(int s) { (void)s; _exit(2); }

static void *sender(void *arg)
{
    long delay = (long)arg;
    usleep((useconds_t)delay);
    kill(getpid(), SIGX);
    return NULL;
}

static int wait_via(int mode, int timeout_ms)
{
    if (mode == 0) {
        sigset_t tmp;
        sigemptyset(&tmp);
        return sigsuspend(&tmp);
    }
    if (mode == 1) {
        struct pollfd pf = { .fd = pipefd[0], .events = POLLIN };
        sigset_t tmp;
        sigemptyset(&tmp);
        return ppoll(&pf, 1, &(struct timespec){timeout_ms, 0}, &tmp);
    }
    int ep = epoll_create1(0);
    struct epoll_event ev = { .events = EPOLLIN, .data.fd = pipefd[0] };
    epoll_ctl(ep, EPOLL_CTL_ADD, pipefd[0], &ev);
    sigset_t tmp;
    sigemptyset(&tmp);
    struct epoll_event out;
    int r = epoll_pwait(ep, &out, 1, timeout_ms, &tmp);
    close(ep);
    return r;
}

int main(void)
{
    int r = race_require_cpus(2, "case15_tmp_mask_lost_wakeup");
    if (r) return r;

    struct sigaction sa = {0}; sa.sa_handler = h_x; sigemptyset(&sa.sa_mask);
    sigaction(SIGX, &sa, NULL);

    sigset_t block;
    sigemptyset(&block);
    sigaddset(&block, SIGX);
    sigprocmask(SIG_BLOCK, &block, NULL);

    if (pipe(pipefd) < 0) { perror("pipe"); return 1; }

    const char *names[] = { "sigsuspend", "ppoll", "epoll_pwait" };
    for (int mode = 0; mode < 3; mode++) {
        int miss = 0;
        for (int round = 0; round < ROUNDS; round++) {
            atomic_store(&x_handled, 0);
            signal(SIGALRM, on_deadline);
            alarm(2);

            pthread_t st;
            long delay = (round % 20) * 100;
            pthread_create(&st, NULL, sender, (void *)delay);

            wait_via(mode, 1500);
            alarm(0);
            pthread_join(st, NULL);

            if (!atomic_load(&x_handled)) {
                char c; write(pipefd[1], &c, 1); read(pipefd[0], &c, 1);
                miss++;
            }
        }
        if (miss)
            printf("case15_tmp_mask_lost_wakeup [%s]: HIT (miss=%d/%d)\n", names[mode], miss, ROUNDS);
        else
            printf("case15_tmp_mask_lost_wakeup [%s]: PASS\n", names[mode]);
        if (miss) atomic_fetch_add(&hit, 1);
    }
    close(pipefd[0]); close(pipefd[1]);
    return 0;
}
