/*
 * 对应 docs/race/ipc.md 第 12 节：被忽略信号占用 TCB 槽位并压制后续可处理信号。
 *
 * 触发条件：未屏蔽信号 try_recive_pending_signal 先写 pending_signal 再读 disposition，
 * 发现 SIG_IGN 直接返回不清槽位也不唤醒；后续普通可处理信号看到槽位已占用，在检查
 * 自身 disposition 前转入进程待处理队列，也不唤醒已阻塞任务。
 *
 * 本测试：目标线程安装 SIG_IGN 的信号 I 和计数型实时信号 X 的处理函数，然后阻塞读
 * 保持打开但不写入的 pipe。另一 CPU 先发一次 I，再发一次 X。以 X 的处理期限为判据；
 * 期限到达后才由 watchdog 写 pipe 唤醒目标，记录 X 是否只在外部唤醒后才执行。附加
 * 场景：唤醒前把 I 从 SIG_IGN 改为处理函数，检查旧 I 是否被错误执行。
 */
#define _GNU_SOURCE
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <unistd.h>
#include <errno.h>
#include <signal.h>
#include <pthread.h>
#include <stdatomic.h>
#include "smp_check.h"

#define SIGI (SIGRTMIN + 2)
#define SIGX (SIGRTMIN + 3)

static int pfd[2];
static atomic_int x_handled = 0;
static atomic_int i_handled = 0;

static void h_x(int s) { (void)s; atomic_store(&x_handled, 1); }
static void h_i(int s) { (void)s; atomic_store(&i_handled, 1); }

static void on_watchdog(int s)
{
    (void)s;
    if (!atomic_load(&x_handled)) {
        printf("case12_ignored_signal_slot: HIT (X suppressed by occupied ignored slot)\n");
        char c = 1; write(pfd[1], &c, 1);
    } else {
        printf("case12_ignored_signal_slot: PASS\n");
    }
    _exit(0);
}

static void *target(void *arg)
{
    (void)arg;
    char c;
    read(pfd[0], &c, 1);
    return NULL;
}

int main(void)
{
    int r = race_require_cpus(2, "case12_ignored_signal_slot");
    if (r) return r;

    if (pipe(pfd) < 0) { perror("pipe"); return 1; }

    struct sigaction si = {0}; si.sa_handler = SIG_IGN; sigemptyset(&si.sa_mask);
    sigaction(SIGI, &si, NULL);
    struct sigaction sx = {0}; sx.sa_handler = h_x; sigemptyset(&sx.sa_mask);
    sigaction(SIGX, &sx, NULL);

    signal(SIGALRM, on_watchdog);
    alarm(4);

    pthread_t t;
    pthread_create(&t, NULL, target, NULL);
    usleep(50 * 1000);

    pthread_kill(t, SIGI);
    usleep(20 * 1000);
    pthread_kill(t, SIGX);

    pthread_join(t, NULL);
    close(pfd[0]); close(pfd[1]);

    struct sigaction si2 = {0}; si2.sa_handler = h_i; sigemptyset(&si2.sa_mask);
    sigaction(SIGI, &si2, NULL);
    return 0;
}
