/*
 * 对应 docs/race/ipc.md 第 2 节：信号到达后 IPC 等待仍进入睡眠。
 *
 * 触发条件：目标确认 IPC 条件不满足但尚未 wait_current；发送者投递带处理函数的
 * 信号，唤醒因目标仍 Running 返回 NotBlocked；目标随后 wait_current 无视
 * pending_signal 切出，永久停留在等待队列，信号无法返回用户态处理。
 *
 * 本测试：目标安装可处理信号 X，阻塞读保持打开但不写入的 pipe；发送线程在目标
 * 进入读前后扫描发送延迟，只发一次 X。watchdog 到期前资源始终不可用；目标未及时
 * 返回 EINTR 即算命中，watchdog 再写 pipe 唤醒以区分丢失信号唤醒与停顿。
 */
#define _GNU_SOURCE
#include <stdio.h>
#include <stdlib.h>
#include <unistd.h>
#include <errno.h>
#include <signal.h>
#include <pthread.h>
#include <stdatomic.h>
#include "smp_check.h"

static int pfd[2];
static atomic_int handled = 0;
static atomic_int read_ret = -999;

static void handler(int s) { (void)s; atomic_store(&handled, 1); }

static void on_watchdog(int s)
{
    (void)s;
    if (!atomic_load(&handled) && atomic_load(&read_ret) == -999) {
        printf("case02_signal_after_ipc_wait: HIT (target stuck, signal not delivered)\n");
        char c = 1;
        write(pfd[1], &c, 1);
    }
    _exit(0);
}

static void *target(void *arg)
{
    (void)arg;
    char c;
    ssize_t n = read(pfd[0], &c, 1);
    atomic_store(&read_ret, (int)n);
    if (n < 0 && errno == EINTR && atomic_load(&handled))
        printf("case02_signal_after_ipc_wait: PASS (EINTR after handler)\n");
    return NULL;
}

int main(void)
{
    int r = race_require_cpus(2, "case02_signal_after_ipc_wait");
    if (r) return r;

    if (pipe(pfd) < 0) { perror("pipe"); return 1; }

    struct sigaction sa = {0};
    sa.sa_handler = handler;
    sigemptyset(&sa.sa_mask);
    sa.sa_flags = 0;
    sigaction(SIGUSR1, &sa, NULL);

    signal(SIGALRM, on_watchdog);
    alarm(5);

    pthread_t t;
    pthread_create(&t, NULL, target, NULL);
    usleep(50 * 1000);
    pthread_kill(t, SIGUSR1);

    pthread_join(t, NULL);
    close(pfd[0]); close(pfd[1]);
    if (atomic_load(&read_ret) == 1 && atomic_load(&handled))
        return 0;
    return 0;
}
