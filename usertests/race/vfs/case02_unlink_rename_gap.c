/*
 * 对应 docs/race/vfs.md 第 2 节（A 支）：Dentry::remove_child / rename 的 BTreeMap
 * 与磁盘操作脱锁，unlink 与 rename 跨同一名字。
 *
 * 触发条件：remove_child 先做 inode.unlink 再做 children.lock().remove + cache.remove，
 * 期间另一线程可对同一名字执行 rename(new_parent.lookup("x") 命中旧 inode，再
 * inode.rename("other","x"))。最终 BTreeMap 与磁盘项长期分离：rename 写入磁盘的 inode
 * 没有对应 dentry，而 children 残留旧引用/被同时 remove 清掉。
 *
 * 本测试：3 线程固定名字池。A 线程循环 unlink(name)；B 线程循环把另一源文件 rename 到
 * 同一 name；C 线程持续 stat/readdir/lookup 该 name 并记录结果。命中：readdir 出现 name 但
 * stat 失败 / 或 readdir 不出现 name 但 stat 成功 / 或 stat 返回 inode 与 Cache::find 不一致导致
 * link/lookup 出错 / 或目录最终残留孤儿条目。
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

#define WORKDIR "/tmp/race_vfs_case02"
#define SRC     WORKDIR "/src"
#define DST     WORKDIR "/dst"

static atomic_int stop = 0;
static atomic_int hit_inconsistent = 0;
static atomic_int hit_orphan = 0;
static atomic_int hit_stat_enoent = 0;

static void on_watchdog(int s) { (void)s; atomic_store(&stop, 1); _exit(0); }

static int ensure_src(void)
{
    int fd = open(SRC, O_CREAT | O_RDWR | O_TRUNC, 0600);
    if (fd < 0) return -1;
    write(fd, "S", 1);
    close(fd);
    return 0;
}

static void *unlinker(void *arg)
{
    (void)arg;
    while (!atomic_load(&stop)) {
        /* 制造窗口：先存在再 unlink，与 rename 抢同名。*/
        if (ensure_src() < 0) continue;
        unlink(DST);
        usleep(50);
        unlink(DST);
    }
    return NULL;
}

static void *renamer(void *arg)
{
    (void)arg;
    while (!atomic_load(&stop)) {
        /* 始终把另一文件改名为 dst，与 unlink 抢同一名字。*/
        if (ensure_src() == 0) {
            rename(SRC, DST);
        }
    }
    return NULL;
}

static void *verifier(void *arg)
{
    (void)arg;
    char buf[64];
    while (!atomic_load(&stop)) {
        int have_dirent = 0;
        DIR *d = opendir(WORKDIR);
        if (d) {
            struct dirent *e;
            while ((e = readdir(d)) != NULL) {
                if (strcmp(e->d_name, "dst") == 0) have_dirent = 1;
            }
            closedir(d);
        }

        struct stat st;
        int have_stat = (stat(DST, &st) == 0);
        int have_open = -1;
        if (have_stat) {
            int fd = open(DST, O_RDONLY);
            if (fd >= 0) {
                buf[0] = 0;
                ssize_t n = read(fd, buf, sizeof(buf) - 1);
                close(fd);
                if (n > 0) buf[n] = 0;
                have_open = (buf[0] == 'S') ? 1 : 0;
            } else {
                have_open = -2; /* stat 成功但 open 失败：高度可疑。*/
            }
        }

        if (have_dirent && !have_stat)
            atomic_fetch_add(&hit_orphan, 1);
        if (!have_dirent && have_stat)
            atomic_fetch_add(&hit_stat_enoent, 1);
        if (have_stat && have_open == 0)
            atomic_fetch_add(&hit_inconsistent, 1); /* stat 看见但内容错乱。*/
        if (have_stat && have_open == -2)
            atomic_fetch_add(&hit_inconsistent, 1);
    }
    return NULL;
}

int main(void)
{
    int r = race_require_cpus(3, "case02_unlink_rename_gap");
    if (r) return r;

    signal(SIGALRM, on_watchdog);
    alarm(10);

    if (mkdir(WORKDIR, 0755) < 0 && errno != EEXIST) { perror("mkdir"); return 1; }
    unlink(SRC); unlink(DST);

    pthread_t a, b, c;
    pthread_create(&a, NULL, unlinker, NULL);
    pthread_create(&b, NULL, renamer, NULL);
    pthread_create(&c, NULL, verifier, NULL);

    usleep(8000000);
    atomic_store(&stop, 1);
    pthread_join(a, NULL); pthread_join(b, NULL); pthread_join(c, NULL);

    unlink(SRC); unlink(DST);
    rmdir(WORKDIR);

    int cnt = atomic_load(&hit_orphan) + atomic_load(&hit_stat_enoent)
            + atomic_load(&hit_inconsistent);
    if (cnt) {
        printf("case02_unlink_rename_gap: HIT (orphan=%d readdir_miss=%d inconsistent=%d)\n",
               atomic_load(&hit_orphan),
               atomic_load(&hit_stat_enoent),
               atomic_load(&hit_inconsistent));
        return 0;
    }
    printf("case02_unlink_rename_gap: PASS\n");
    return 0;
}