/*
 * 对应 docs/race/vfs.md 第 2 节（B 支）：两个 rename 把不同源同时改名为同一目标，
 * 全程未持有父 dentry 的 children 锁。
 *
 * 触发条件：Dentry::rename 在 inode.rename 之前做 new_parent.lookup(new_name) 的探测，
 * 期间另一个 rename 路径也探测到目标不存在（overwritten=None），二者都执行 inode.rename，
 * 最后两次 children.lock().remove(new_name)，第二次抹掉第一次刚建立的 BTreeMap 条目，
 * 但磁盘上目标已经引用某个源 inode；同时 Cache::remove 没机会清理另一个源的 inode。
 *
 * 本测试：两个 renamer 线程在每轮 coherent barrier 下同步对各自源文件 (A/B) rename 到
 * 同一目标名 dst，barrier 之后再放开 verifier 检查“视窗”中 readdir/stat 的一致性。这样
 * 避免运行期 verifier 被并发 rename 干扰制造假阳性，命中只能是 dentry 缓存与磁盘目录
 * 项分离导致的孤儿条目 / stat 失败但 readdir 命中 / dst 内容既不是 A 也不是 B。
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

#include "smp_check.h"

#define WORKDIR "/tmp/race_vfs_case03"
#define PA      WORKDIR "/a"
#define PB      WORKDIR "/b"
#define PDST    WORKDIR "/dst"

#define ROUNDS 1000

static pthread_barrier_t bar;
static atomic_int hit_dup_dirent = 0;
static atomic_int hit_orphan_dirent = 0;
static atomic_int hit_inconsistent = 0;

static void on_watchdog(int s) { (void)s; _exit(0); }

static int touch(const char *p, char tag)
{
    int fd = open(p, O_CREAT | O_RDWR | O_TRUNC, 0600);
    if (fd < 0) return -1;
    write(fd, &tag, 1);
    close(fd);
    return 0;
}

static void *renamer(void *arg)
{
    char tag = (char)(long)arg;
    const char *src = (tag == 'A') ? PA : PB;
    while (1) {
        if (touch(src, tag) < 0) { _exit(2); }
        /* 准备就绪，与对手和 verifier 在 barrier 决胜。*/
        pthread_barrier_wait(&bar);
        rename(src, PDST);
        pthread_barrier_wait(&bar);
        /* verifier 检查期间此线程空闲。*/
        pthread_barrier_wait(&bar);
    }
    return NULL;
}

static void *verifier(void *arg)
{
    (void)arg;
    while (1) {
        pthread_barrier_wait(&bar);
        pthread_barrier_wait(&bar);

        /* 视窗：所有 renamer 此刻空闲，readdir 与 stat 必须一致。*/
        int dirent_cnt = 0;
        DIR *d = opendir(WORKDIR);
        if (d) {
            struct dirent *e;
            while ((e = readdir(d)) != NULL) {
                if (strcmp(e->d_name, "dst") == 0) dirent_cnt++;
                if (strcmp(e->d_name, "a") == 0) dirent_cnt++;
                if (strcmp(e->d_name, "b") == 0) dirent_cnt++;
            }
            closedir(d);
        }
        if (dirent_cnt > 1)
            atomic_fetch_add(&hit_dup_dirent, 1);

        struct stat st;
        int have_stat = (stat(PDST, &st) == 0);
        if (have_stat) {
            int fd = open(PDST, O_RDONLY);
            if (fd < 0) {
                /* stat 成功但 open 失败：dentry 缓存指向已 free 的 inode。*/
                atomic_fetch_add(&hit_orphan_dirent, 1);
            } else {
                char buf[2] = {0, 0};
                ssize_t n = read(fd, buf, 1);
                close(fd);
                if (n != 1 || (buf[0] != 'A' && buf[0] != 'B'))
                    atomic_fetch_add(&hit_inconsistent, 1);
            }
            unlink(PDST);
        }
        /* 清理源文件残留，下一轮重新 touch。*/
        unlink(PA);
        unlink(PB);
        pthread_barrier_wait(&bar);
    }
    return NULL;
}

int main(void)
{
    int r = race_require_cpus(3, "case03_rename_same_target");
    if (r) return r;

    signal(SIGALRM, on_watchdog);
    alarm(12);

    if (mkdir(WORKDIR, 0755) < 0 && errno != EEXIST) { perror("mkdir"); return 1; }
    unlink(PA); unlink(PB); unlink(PDST);

    if (pthread_barrier_init(&bar, NULL, 3) != 0) { perror("barrier"); return 1; }

    pthread_t a, b, c;
    pthread_create(&a, NULL, renamer, (void *)(long)'A');
    pthread_create(&b, NULL, renamer, (void *)(long)'B');
    pthread_create(&c, NULL, verifier, NULL);

    /* 由 watchdog alarm 超时退出。*/
    pthread_join(a, NULL); pthread_join(b, NULL); pthread_join(c, NULL);
    pthread_barrier_destroy(&bar);

    unlink(PA); unlink(PB); unlink(PDST);
    rmdir(WORKDIR);

    int total = atomic_load(&hit_dup_dirent)
              + atomic_load(&hit_orphan_dirent)
              + atomic_load(&hit_inconsistent);
    if (total) {
        printf("case03_rename_same_target: HIT (dup_dirent=%d orphan=%d inconsistent=%d)\n",
               atomic_load(&hit_dup_dirent),
               atomic_load(&hit_orphan_dirent),
               atomic_load(&hit_inconsistent));
        return 0;
    }
    printf("case03_rename_same_target: PASS\n");
    return 0;
}