/*
 * 对应 docs/race/ipc.md 第 3 节：semtimedop 在安装定时器前到期后遗留 Blocked 状态。
 *
 * 触发条件：不可满足的 semop（值为 0 的信号量减操作），零或极短超时。begin_semop
 * 先把任务设为 Blocked 并入队，返回系统调用层时 deadline 已过，只 remove 等待项
 * 并返回 EAGAIN，未把 TCB 恢复 Running。任务以 Blocked 继续返回路径，下次调度消失。
 *
 * 本测试：先用零超时验证确定性路径，再在最小正值附近分档扫描。每次 EAGAIN 后递增
 * 进度计数并 sched_yield；watchdog 期限内进度永久停止即算命中。
 */
#define _GNU_SOURCE
#include <stdio.h>
#include <stdlib.h>
#include <unistd.h>
#include <errno.h>
#include <signal.h>
#include <sched.h>
#include <sys/types.h>
#include <sys/ipc.h>
#include <sys/sem.h>
#include <stdatomic.h>

enum {
    TEST_ROUNDS = 20000,
    TEST_TIMEOUT_SECONDS = 120,
    TEST_TIMEOUT_EXIT_CODE = 124,
};

static atomic_int progress = 0;

static void on_watchdog(int s)
{
    (void)s;
    int p = atomic_load(&progress);
    printf("case03_semtimedop_blocked_leak: TIMEOUT (progress=%d/%d, stuck Blocked)\n",
           p, TEST_ROUNDS);
    _exit(TEST_TIMEOUT_EXIT_CODE);
}

int main(void)
{
    int semid = semget(IPC_PRIVATE, 1, 0600 | IPC_CREAT);
    if (semid < 0) { perror("semget"); return 1; }

    signal(SIGALRM, on_watchdog);
    alarm(TEST_TIMEOUT_SECONDS);

    struct sembuf op = { .sem_num = 0, .sem_op = -1, .sem_flg = 0 };
    struct timespec timeouts[] = {
        {0, 0}, {0, 1000}, {0, 10000}, {0, 100000}, {0, 500000},
    };

    for (int round = 0; round < TEST_ROUNDS; round++) {
        const struct timespec *to = &timeouts[round % (int)(sizeof(timeouts)/sizeof(timeouts[0]))];
        int r = semtimedop(semid, &op, 1, to);
        if (r == 0) { fprintf(stderr, "unexpected success\n"); semctl(semid, 0, IPC_RMID); return 1; }
        if (errno != EAGAIN) { perror("semtimedop"); semctl(semid, 0, IPC_RMID); return 1; }
        atomic_fetch_add(&progress, 1);
        sched_yield();
    }

    semctl(semid, 0, IPC_RMID);
    alarm(0);
    printf("case03_semtimedop_blocked_leak: PASS (progress=%d)\n", atomic_load(&progress));
    return 0;
}
