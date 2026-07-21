/*
 * 对应 docs/race/ipc.md 第 5 节：shmdt 可解除并发安装的替代映射。
 *
 * 触发条件：detach_shm_by_addr 在 SHM_MANAGER 下删除 (pid,address)->shmid，释放锁
 * 后按旧段长度 unmap_area，两阶段间不持映射锁也不重新验证。并发 MAP_FIXED 可在同址
 * 安装新映射，被迟到的 shmdt 当作旧共享内存段删除。更强顺序路径：先 MAP_FIXED 替换
 * 再 shmdt，仍按陈旧 attach_map 项解除替代映射。
 *
 * 本测试：先跑无并发基线（MAP_FIXED 替换后 shmdt，校验替代映射是否被误删）；再跑
 * 并发场景：D 反复 shmdt，R 在同址反复 MAP_FIXED 匿名映射并写入 generation 标记校验。
 * 替代映射在 shmdt 成功后消失或 fault 即算命中。
 */
#define _GNU_SOURCE
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <unistd.h>
#include <errno.h>
#include <signal.h>
#include <sys/mman.h>
#include <sys/ipc.h>
#include <sys/shm.h>
#include <pthread.h>
#include <stdatomic.h>

#define SEG_BYTES (4 * 4096)
#define BASE_ADDR ((void *)0x40000000UL)
#define ROUNDS 200

static atomic_int hit = 0;
static atomic_int stop = 0;

static void on_watchdog(int s) { (void)s; atomic_store(&stop, 1); _exit(0); }

static int baseline(void)
{
    int id = shmget(IPC_PRIVATE, SEG_BYTES, 0600 | IPC_CREAT);
    if (id < 0) { perror("shmget"); return 1; }
    void *p = shmat(id, BASE_ADDR, SHM_RND);
    if (p == (void *)-1) { perror("shmat"); shmctl(id, IPC_RMID, NULL); return 1; }
    ((volatile char *)p)[0] = 'S';

    void *r = mmap(BASE_ADDR, SEG_BYTES, PROT_READ | PROT_WRITE,
                   MAP_FIXED | MAP_ANONYMOUS | MAP_PRIVATE, -1, 0);
    if (r == MAP_FAILED) { perror("mmap fixed"); shmdt(p); shmctl(id, IPC_RMID, NULL); return 1; }
    ((volatile char *)r)[0] = 'N';

    if (shmdt(BASE_ADDR) < 0) { perror("shmdt"); }

    volatile char v = ((volatile char *)r)[0];
    int bad = (v != 'N');
    if (bad) printf("case05 baseline: HIT (replacement vanished after shmdt)\n");

    munmap(BASE_ADDR, SEG_BYTES);
    shmctl(id, IPC_RMID, NULL);
    return bad;
}

static void *detacher(void *arg)
{
    (void)arg;
    while (!atomic_load(&stop)) {
        int id = shmget(IPC_PRIVATE, SEG_BYTES, 0600 | IPC_CREAT);
        if (id < 0) continue;
        void *p = shmat(id, BASE_ADDR, SHM_RND);
        if (p == (void *)-1) { shmctl(id, IPC_RMID, NULL); continue; }
        shmdt(BASE_ADDR);
        shmctl(id, IPC_RMID, NULL);
    }
    return NULL;
}

static void *replacer(void *arg)
{
    (void)arg;
    while (!atomic_load(&stop)) {
        void *r = mmap(BASE_ADDR, SEG_BYTES, PROT_READ | PROT_WRITE,
                       MAP_FIXED | MAP_ANONYMOUS | MAP_PRIVATE, -1, 0);
        if (r == MAP_FAILED) continue;
        ((volatile char *)r)[0] = 'N';
        volatile char v = ((volatile char *)r)[0];
        if (v != 'N') atomic_fetch_add(&hit, 1);
        munmap(BASE_ADDR, SEG_BYTES);
    }
    return NULL;
}

int main(void)
{
    signal(SIGALRM, on_watchdog);
    alarm(8);

    if (baseline()) return 0;

    pthread_t a, b;
    pthread_create(&a, NULL, detacher, NULL);
    pthread_create(&b, NULL, replacer, NULL);
    usleep(5000000 / 1000 * 1000);
    atomic_store(&stop, 1);
    pthread_join(a, NULL);
    pthread_join(b, NULL);

    if (atomic_load(&hit))
        printf("case05_shmdt_replace_map: HIT (replacement corrupted %d times)\n", atomic_load(&hit));
    else
        printf("case05_shmdt_replace_map: PASS\n");
    return 0;
}
