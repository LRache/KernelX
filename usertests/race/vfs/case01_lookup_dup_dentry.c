/*
 * 对应 docs/race/vfs.md 第 1 节：Dentry::lookup_with_perm 双段缓存插入产生孤儿 dentry。
 *
 * 触发条件：lookup_with_perm 先在 children.lock() 下做命中检查，未命中则在无锁状态下
 * 执行 inode.lookup 与 Arc::new(Dentry)，再取一次 children.lock() 做后者者胜插入。两个
 * 线程同时未命中同一名字时，落败线程抛掉自己刚构造的 Arc；但若它在此之前已经把该
 * dentry 作为 bind/move 锚点或下一层 lookup 的起点，后续路径就会与 children 中存活的
 * dentry 出现身份不一致。
 *
 * 本测试不涉及 mount，仅复现“同一 (sno,ino,name) 下并发 lookup 产生的可观察不一致”：
 * 多线程以 barrier 同步对同一名字同时发起 O_CREAT|O_EXCL，胜者创建文件，败者得到 EEXIST
 * 但仍按 inode lookup 的“刚加载”路径返回 dentry。随后各线程对该 dentry 做 link/readdir/
 * stat 验证。最关键一致性：相同 st_ino + st_dev 出现两种以上 st_nlink 行为、readdir 中
 * 出现重复条目、或 lookup 后 link 同名失败但 stat 仍成功即命中。
 *
 * 命中条件：到一轮结束出现以下之一：
 *   - 两个线程同时 `O_CREAT|O_EXCL` 同一名字都成功（真重复创建）；
 *   - readdir 在目录中出现重复的同名条目；
 *   - 同时持有同 ino 的 dentry 走 link 同名得到非 EEXIST 的结果；
 *   - kernel panic / lookup 返回的 st_ino 与 stat/`/proc/self/...` 给出的不一致。
 */
#define _GNU_SOURCE
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <unistd.h>
#include <fcntl.h>
#include <errno.h>
#include <signal.h>
#include <dirent.h>
#include <pthread.h>
#include <stdatomic.h>
#include <sys/stat.h>
#include <sys/types.h>

#include "smp_check.h"

#define WORKDIR "/tmp/race_vfs_case01"
#define THREADS 4
#define ITERS 600

static pthread_barrier_t bar;
static atomic_int stop = 0;
static atomic_int dup_create = 0;
static atomic_int dup_dirent = 0;
static atomic_int link_odd = 0;
static atomic_int stat_mismatch = 0;

static void on_watchdog(int s) { (void)s; atomic_store(&stop, 1); _exit(0); }

static int count_dirent_name(const char *dir, const char *name)
{
    DIR *d = opendir(dir);
    if (!d) return -1;
    int cnt = 0;
    struct dirent *e;
    while ((e = readdir(d)) != NULL) {
        if (strcmp(e->d_name, name) == 0) cnt++;
    }
    closedir(d);
    return cnt;
}

static void *worker(void *arg)
{
    int tid = (int)(long)arg;
    char path[256];
    char linkpath[256];

    for (int i = 0; i < ITERS && !atomic_load(&stop); i++) {
        /* 每轮用同一个固定名字，强制所有线程同时未命中。*/
        const char *name = "shared";
        snprintf(path, sizeof(path), "%s/%s", WORKDIR, name);
        snprintf(linkpath, sizeof(linkpath), "%s/link_%d_%d", WORKDIR, tid, i);

        /* 同步所有线程同时发起 lookup+create，最大化双段插入窗口。*/
        pthread_barrier_wait(&bar);

        int fd = open(path, O_CREAT | O_EXCL | O_RDWR, 0600);
        int created = (fd >= 0);
        if (fd >= 0) {
            close(fd);
        } else if (errno != EEXIST) {
            /* 不是 EEXIST 也不是成功：路径异常，记为不一致。*/
            atomic_fetch_add(&stat_mismatch, 1);
            pthread_barrier_wait(&bar);
            continue;
        }

        /* 全线程在创建后再次 lookup 同名，争引 inode lookup 路径。*/
        int fd2 = open(path, O_RDONLY);
        if (fd2 < 0) {
            atomic_fetch_add(&stat_mismatch, 1);
        } else {
            struct stat st;
            if (fstat(fd2, &st) < 0 || st.st_nlink == 0)
                atomic_fetch_add(&stat_mismatch, 1);
            close(fd2);
        }

        /* 同时尝试对同名做硬链接，胜者应得到 EEXIST（已存在），败者也应得到 EEXIST。
         * 若返回 0，意味着内核把指向旧 inode 的 dentry 挂到一个已不存在的新 dentry 上。*/
        if (link(path, linkpath) == 0) {
            atomic_fetch_add(&link_odd, 1);
            unlink(linkpath);
        } else if (errno != EEXIST) {
            atomic_fetch_add(&link_odd, 1);
        }

        pthread_barrier_wait(&bar);

        /* 检查目录内同名条目数量。O_CREAT|O_EXCL 不会创建大于条目。*/
        int cnt = count_dirent_name(WORKDIR, name);
        if (cnt > 1)
            atomic_fetch_add(&dup_dirent, 1);
        if (created && cnt == 1) {
            /* 正常：创建者唯一，readdir 一次命中。*/
        }
        if (created) atomic_fetch_add(&dup_create, 1);

        /* 清场：所有线程都 unlink 自己可能创建的同名。*/
        unlink(path);
        unlink(linkpath);

        pthread_barrier_wait(&bar);
    }
    return NULL;
}

int main(void)
{
    int r = race_require_cpus(2, "case01_lookup_dup_dentry");
    if (r) return r;

    signal(SIGALRM, on_watchdog);
    alarm(20);

    if (mkdir(WORKDIR, 0755) < 0 && errno != EEXIST) { perror("mkdir"); return 1; }
    /* 清掉工作目录残留。*/
    DIR *d = opendir(WORKDIR);
    if (d) {
        struct dirent *e;
        char buf[256];
        while ((e = readdir(d)) != NULL) {
            if (strcmp(e->d_name, ".") == 0 || strcmp(e->d_name, "..") == 0) continue;
            snprintf(buf, sizeof(buf), "%s/%s", WORKDIR, e->d_name);
            unlink(buf);
        }
        closedir(d);
    }

    if (pthread_barrier_init(&bar, NULL, THREADS) != 0) { perror("barrier"); return 1; }

    pthread_t th[THREADS];
    for (int i = 0; i < THREADS; i++)
        if (pthread_create(&th[i], NULL, worker, (void *)(long)i)) { perror("pthread"); return 1; }
    for (int i = 0; i < THREADS; i++) pthread_join(th[i], NULL);
    pthread_barrier_destroy(&bar);

    /* 最终清场。*/
    d = opendir(WORKDIR);
    if (d) {
        struct dirent *e;
        char buf[256];
        while ((e = readdir(d)) != NULL) {
            if (strcmp(e->d_name, ".") == 0 || strcmp(e->d_name, "..") == 0) continue;
            snprintf(buf, sizeof(buf), "%s/%s", WORKDIR, e->d_name);
            unlink(buf);
        }
        closedir(d);
    }
    rmdir(WORKDIR);

    int dc = atomic_load(&dup_create);
    int dd = atomic_load(&dup_dirent);
    int lo = atomic_load(&link_odd);
    int sm = atomic_load(&stat_mismatch);
    if (dc != ITERS) {
        fprintf(stderr, "FAIL: created=%d expected=%d dd=%d lo=%d sm=%d\n", dc, ITERS, dd, lo, sm);
        return 1;
    }
    if (dd || lo || sm) {
        printf("case01_lookup_dup_dentry: HIT (dup_dirent=%d link_odd=%d stat_mismatch=%d)\n",
               dd, lo, sm);
        return 0;
    }
    printf("case01_lookup_dup_dentry: PASS\n");
    return 0;
}