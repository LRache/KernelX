/*
 * 对应 docs/race/ipc.md 第 14 节：信号出队与处理函数屏蔽集安装之间存在嵌套窗口。
 *
 * 触发条件：handle_signal 取走 pending_signal 并释放状态锁后才读 action 并安装
 * sa_mask 及默认自身屏蔽；另一 CPU 可在该窗口按旧 mask 把同号信号或 sa_mask 中的
 * 信号写入刚空出的槽位。后续 handle_signal 消费槽位不重新检查当前 mask，可在没有
 * SA_NODEFER 时嵌套处理。
 *
 * 本测试：安装不带 SA_NODEFER 的长执行 X 处理函数，用原子深度计数检测嵌套；多个 CPU
 * 高频向同一 TID 发送 X，处理函数内部周期性执行系统调用增加返回用户态次数。另设场景
 * 让信号 Y 只出现在 X 的 sa_mask 中，并在 X 处理期间高频发送 Y。处理深度 > 1，或 Y
 * 在 X 返回前执行，即算命中。注意：窄窗口需真正 SMP 才易触发。
 */
#define _GNU_SOURCE
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <unistd.h>
#include <errno.h>
#include <signal.h>
#include <pthread.h>
#include <sys/syscall.h>
#include <stdatomic.h>
#include "smp_check.h"

#define SIGX (SIGRTMIN + 5)
#define SIGY (SIGRTMIN + 6)
#define SENDERS 4
#define DURATION_SEC 3

static atomic_int depth = 0;
static atomic_int max_depth = 0;
static atomic_int y_in_x = 0;
static atomic_int in_x = 0;
static atomic_int stop = 0;

static void h_x(int s)
{
    (void)s;
    int d = atomic_fetch_add(&depth, 1) + 1;
    int m = atomic_load(&max_depth);
    while (d > m) m = atomic_compare_exchange_strong(&max_depth, &m, d) ? d : atomic_load(&max_depth);
    atomic_store(&in_x, 1);
    for (volatile int i = 0; i < 200000; i++) { syscall(SYS_getpid); }
    atomic_store(&in_x, 0);
    atomic_fetch_sub(&depth, 1);
}

static void h_y(int s)
{
    (void)s;
    if (atomic_load(&in_x)) atomic_fetch_add(&y_in_x, 1);
}

static void *sender_x(void *arg)
{
    (void)arg;
    pid_t self = getpid();
    while (!atomic_load(&stop)) { kill(self, SIGX); usleep(200); }
    return NULL;
}

static void *sender_y(void *arg)
{
    (void)arg;
    pid_t self = getpid();
    while (!atomic_load(&stop)) { kill(self, SIGY); usleep(200); }
    return NULL;
}

int main(void)
{
    int r = race_require_cpus(2, "case14_signal_mask_nest");
    if (r) return r;

    struct sigaction sx = {0};
    sx.sa_handler = h_x;
    sigemptyset(&sx.sa_mask);
    sigaddset(&sx.sa_mask, SIGY);
    sigaddset(&sx.sa_mask, SIGX);
    sx.sa_flags = 0;
    sigaction(SIGX, &sx, NULL);

    struct sigaction sy = {0};
    sy.sa_handler = h_y;
    sigemptyset(&sy.sa_mask);
    sigaction(SIGY, &sy, NULL);

    alarm(DURATION_SEC + 1);
    pthread_t tx[SENDERS], ty[SENDERS];
    for (int i = 0; i < SENDERS; i++) pthread_create(&tx[i], NULL, sender_x, NULL);
    for (int i = 0; i < SENDERS; i++) pthread_create(&ty[i], NULL, sender_y, NULL);

    sleep(DURATION_SEC);
    atomic_store(&stop, 1);

    for (int i = 0; i < SENDERS; i++) { pthread_join(tx[i], NULL); pthread_join(ty[i], NULL); }

    if (atomic_load(&max_depth) > 1 || atomic_load(&y_in_x) > 0)
        printf("case14_signal_mask_nest: HIT (max_depth=%d y_in_x=%d)\n",
               atomic_load(&max_depth), atomic_load(&y_in_x));
    else
        printf("case14_signal_mask_nest: PASS (max_depth=%d y_in_x=%d)\n",
               atomic_load(&max_depth), atomic_load(&y_in_x));
    return 0;
}
