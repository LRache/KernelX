/*
 * 对应 docs/race/ipc.md 第 4 节：SHM_MANAGER 与地址空间映射锁形成 ABBA 死锁。
 *
 * 触发条件：shmat 持有 SHM_MANAGER 后获取 map_manager；fork/munmap/MAP_FIXED 替换
 * 先持有 map_manager 再经 ShmArea::fork/drop 获取 SHM_MANAGER，两个共享地址空间
 * 线程形成闭合锁环。
 *
 * 本测试：多线程进程预创建并附加多个共享内存段；一组线程反复 shmat 新段，一组反复
 * fork，一组对已有段执行整段 munmap + 重新 shmat。watchdog 监控各组进度；任一组
 * 超时无进展即算命中（死锁）。当前 lockdep 关闭，仅靠进度判定。
 */
#define _GNU_SOURCE
#include <stdio.h>
#include <stdlib.h>
#include <unistd.h>
#include <errno.h>
#include <signal.h>
#include <sys/types.h>
#include <sys/ipc.h>
#include <sys/shm.h>
#include <sys/mman.h>
#include <sys/wait.h>
#include <pthread.h>
#include <stdatomic.h>
#include "smp_check.h"

#define SEG_BYTES (4 * 4096)
#define SEG_COUNT 4
#define ROUNDS    200

static int shmids[SEG_COUNT];
static atomic_int attach_prog = 0;
static atomic_int fork_prog = 0;
static atomic_int munmap_prog = 0;
static atomic_int stop = 0;

static void on_watchdog(int s)
{
    (void)s;
    atomic_store(&stop, 1);
    int a = atomic_load(&attach_prog), f = atomic_load(&fork_prog), m = atomic_load(&munmap_prog);
    if (a == 0 || f == 0 || m == 0) {
        printf("case04_shm_map_abba: HIT (deadlock a=%d f=%d m=%d)\n", a, f, m);
        _exit(0);
    }
    printf("case04_shm_map_abba: PASS (a=%d f=%d m=%d)\n", a, f, m);
    _exit(0);
}

static void *attacher(void *arg)
{
    (void)arg;
    while (!atomic_load(&stop)) {
        int id = shmget(IPC_PRIVATE, SEG_BYTES, 0600 | IPC_CREAT);
        if (id < 0) { perror("shmget"); break; }
        void *p = shmat(id, NULL, 0);
        if (p == (void *)-1) { perror("shmat"); shmctl(id, IPC_RMID, NULL); break; }
        ((volatile char *)p)[0] = (char)atomic_fetch_add(&attach_prog, 1);
        shmdt(p);
        shmctl(id, IPC_RMID, NULL);
    }
    return NULL;
}

static void *forker(void *arg)
{
    (void)arg;
    while (!atomic_load(&stop)) {
        pid_t pid = fork();
        if (pid == 0) _exit(0);
        if (pid > 0) waitpid(pid, NULL, 0);
        atomic_fetch_add(&fork_prog, 1);
    }
    return NULL;
}

static void *remover(void *arg)
{
    (void)arg;
    while (!atomic_load(&stop)) {
        for (int i = 0; i < SEG_COUNT; i++) {
            void *p = shmat(shmids[i], NULL, 0);
            if (p == (void *)-1) continue;
            munmap(p, SEG_BYTES);
            atomic_fetch_add(&munmap_prog, 1);
        }
    }
    return NULL;
}

int main(void)
{
    int r = race_require_cpus(2, "case04_shm_map_abba");
    if (r) return r;

    signal(SIGALRM, on_watchdog);
    alarm(10);

    for (int i = 0; i < SEG_COUNT; i++) {
        shmids[i] = shmget(IPC_PRIVATE, SEG_BYTES, 0600 | IPC_CREAT);
        if (shmids[i] < 0) { perror("shmget"); return 1; }
        void *p = shmat(shmids[i], NULL, 0);
        if (p == (void *)-1) { perror("shmat"); return 1; }
    }

    pthread_t t[6];
    pthread_create(&t[0], NULL, attacher, NULL);
    pthread_create(&t[1], NULL, attacher, NULL);
    pthread_create(&t[2], NULL, forker, NULL);
    pthread_create(&t[3], NULL, forker, NULL);
    pthread_create(&t[4], NULL, remover, NULL);
    pthread_create(&t[5], NULL, remover, NULL);

    for (int i = 0; i < 6; i++) pthread_join(t[i], NULL);

    for (int i = 0; i < SEG_COUNT; i++) shmctl(shmids[i], IPC_RMID, NULL);
    return 0;
}
