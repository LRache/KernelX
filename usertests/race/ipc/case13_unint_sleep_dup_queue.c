/*
 * 对应 docs/race/ipc.md 第 13 节：不可中断睡眠中的一次信号被同时保存在两个队列。
 *
 * 触发条件：信号直接投递先写 TCB pending_signal 再尝试唤醒；目标为 BlockedUninterruptible
 * 时唤醒失败但直接投递不回滚已写槽位；PCB::send_signal 又把同一 PendingSignal 加入
 * 进程待处理队列。目标恢复后先处理 TCB 槽位中的信号，下次返回用户态再从 PCB 队列取
 * 出同一个信号，导致同一 payload 处理两次。
 *
 * 本测试：父线程安装带计数和唯一 payload 记录的实时信号处理函数，然后执行由子进程
 * 延迟完成的 vfork。确认父线程处于不可中断阶段后只发送一次目标信号，再允许子进程
 * 退出。每轮检查该 payload 处理次数必须恰为一次；同一 payload 两次即算命中。rt_tgsigqueueinfo
 * 缺失，用进程定向 sigqueue 投递；用单线程 vfork 父进程。
 */
#define _GNU_SOURCE
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <unistd.h>
#include <errno.h>
#include <signal.h>
#include <sys/wait.h>
#include <stdatomic.h>

#define SIGX (SIGRTMIN + 4)
#define ROUNDS 200

static atomic_int count = 0;
static volatile sig_atomic_t last_payload = 0;

static void handler(int s, siginfo_t *info, void *u)
{
    (void)s; (void)u;
    if (info) {
        int p = info->si_value.sival_int;
        if (p == last_payload) atomic_fetch_add(&count, 1);
        else { last_payload = p; atomic_store(&count, 1); }
    }
}

int main(void)
{
    struct sigaction sa = {0};
    sa.sa_sigaction = handler;
    sigemptyset(&sa.sa_mask);
    sa.sa_flags = SA_SIGINFO;
    sigaction(SIGX, &sa, NULL);

    volatile int hit = 0;
    for (volatile int round = 0; round < ROUNDS; round++) {
        atomic_store(&count, 0);
        last_payload = round;

        pid_t pid = vfork();
        if (pid == 0) {
            volatile int r = round;
            usleep((r % 5) * 1000 + 100);
            sigqueue(getppid(), SIGX, (union sigval){ .sival_int = r });
            _exit(0);
        }
        waitpid(pid, NULL, 0);

        if (atomic_load(&count) > 1) hit++;
    }
    if (hit)
        printf("case13_unint_sleep_dup_queue: HIT (dup delivery %d/%d)\n", hit, ROUNDS);
    else
        printf("case13_unint_sleep_dup_queue: PASS\n");
    return 0;
}
