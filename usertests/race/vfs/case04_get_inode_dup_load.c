/*
 * 对应 docs/race/vfs.md 第 4 节：Dentry::get_inode 惰性加载的重复 load_inode。
 *
 * 触发条件：get_inode 先 inode.lock() 看 Weak::upgrade()，为 None 时释放锁调用
 * vfs().load_inode(sno, ino)，再取一次 inode.lock() 写入新 Weak。两个线程同时遇到 None 时
 * 各自调用 load_inode -> superblock.get_inode -> cache.insert。最终 Dentry.inode 可能写入
 * 两个不同的 Arc，且被 cache 丢弃的那个在下次 get_inode 又会重新加载，残留状态不一致。
 *
 * 本测试：尝试触发该窗口并验证可观测症状。INODE_CACHE_HIGH_WATERMARK 默认 512，
 * 因此构造 1024 个独立文件作为压力集，由“驱逐者”线程持续 open/close 滚动推送超水位，
 * 同时“受害者”线程固定一组目标文件反复 open/close/stat/lseek 但在每次切换前先短暂释放
 * 全部对该 inode 的引用（通过 close + unlinked tmpfs inode 的非常驻策略），制造 get_inode
 * 第一次走 Weak::upgrade()=None 的机会；多个受害者线程并发同 ino 调用而得到不一致。
 *
 * 命中：内核 panic / 同一文件 stat 在某线程中失败但 open 成功 / 文件读出的内容出现
 * 字符级混淆 / lseek 后 read 返回不该出现的数据 / 重复加载后 read 失败。
 */
#define _GNU_SOURCE
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <unistd.h>
#include <fcntl.h>
#include <errno.h>
#include <signal.h>
#include <pthread.h>
#include <stdatomic.h>
#include <sys/stat.h>
#include <sys/types.h>

#include "smp_check.h"

#define WORKDIR "/tmp/race_vfs_case04"
#define VICTIMS 8
#define PRESSURE 1024

static atomic_int stop = 0;
static atomic_int hit_read_corrupt = 0;
static atomic_int hit_stat_open_mismatch = 0;
static atomic_int hit_read_fail = 0;

static void on_watchdog(int s) { (void)s; atomic_store(&stop, 1); _exit(0); }

static const char *victim_name(int i)
{
    static __thread char buf[64];
    snprintf(buf, sizeof(buf), "%s/v%d", WORKDIR, i);
    return buf;
}

static void *pressurer(void *arg)
{
    (void)arg;
    char path[256];
    while (!atomic_load(&stop)) {
        for (int i = 0; i < PRESSURE; i++) {
            snprintf(path, sizeof(path), "%s/p%d", WORKDIR, i);
            int fd = open(path, O_CREAT | O_RDWR | O_TRUNC, 0600);
            if (fd < 0) continue;
            char c = (char)(i & 0xff);
            write(fd, &c, 1);
            close(fd);
        }
        for (int i = 0; i < PRESSURE; i++) {
            snprintf(path, sizeof(path), "%s/p%d", WORKDIR, i);
            unlink(path);
        }
    }
    return NULL;
}

static void *victim(void *arg)
{
    int id = (int)(long)arg;
    const char tag = 'V' + (char)id;
    char exp[64];
    memset(exp, tag, sizeof(exp));

    while (!atomic_load(&stop)) {
        const char *p = victim_name(id);
        int fd = open(p, O_CREAT | O_RDWR | O_TRUNC, 0600);
        if (fd < 0) continue;
        write(fd, exp, sizeof(exp));
        lseek(fd, 0, SEEK_SET);

        /* stat/open 不一致：get_inode 返回不同 Arc 时，stat 与 read 可能错配 inode。*/
        struct stat st;
        if (fstat(fd, &st) < 0) {
            atomic_fetch_add(&hit_stat_open_mismatch, 1);
            close(fd);
            continue;
        }
        char buf[64];
        ssize_t n = read(fd, buf, sizeof(buf));
        if (n != (ssize_t)sizeof(buf)) {
            atomic_fetch_add(&hit_read_fail, 1);
            close(fd);
            continue;
        }
        /* 内容必须严格等于 exp。如果 read 拿到了别的 inode，建议字节就会错。*/
        if (memcmp(buf, exp, sizeof(buf)) != 0)
            atomic_fetch_add(&hit_read_corrupt, 1);
        close(fd);

        /* unlink 再重开，触发 Dentry::get_inode 重新加载路径。*/
        unlink(p);
    }
    return NULL;
}

int main(void)
{
    int r = race_require_cpus(2, "case04_get_inode_dup_load");
    if (r) return r;

    signal(SIGALRM, on_watchdog);
    alarm(15);

    if (mkdir(WORKDIR, 0755) < 0 && errno != EEXIST) { perror("mkdir"); return 1; }

    pthread_t p, vth[VICTIMS];
    pthread_create(&p, NULL, pressurer, NULL);
    for (int i = 0; i < VICTIMS; i++)
        pthread_create(&vth[i], NULL, victim, (void *)(long)i);

    usleep(12000000);
    atomic_store(&stop, 1);
    pthread_join(p, NULL);
    for (int i = 0; i < VICTIMS; i++) pthread_join(vth[i], NULL);

    /* 清场。*/
    char path[256];
    for (int i = 0; i < PRESSURE; i++) {
        snprintf(path, sizeof(path), "%s/p%d", WORKDIR, i);
        unlink(path);
    }
    for (int i = 0; i < VICTIMS; i++) {
        snprintf(path, sizeof(path), "%s/v%d", WORKDIR, i);
        unlink(path);
    }
    rmdir(WORKDIR);

    int total = atomic_load(&hit_read_corrupt)
              + atomic_load(&hit_stat_open_mismatch)
              + atomic_load(&hit_read_fail);
    if (total) {
        printf("case04_get_inode_dup_load: HIT (read_corrupt=%d stat_open=%d read_fail=%d)\n",
               atomic_load(&hit_read_corrupt),
               atomic_load(&hit_stat_open_mismatch),
               atomic_load(&hit_read_fail));
        return 0;
    }
    printf("case04_get_inode_dup_load: PASS\n");
    return 0;
}