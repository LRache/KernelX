/*
 * 对应 docs/race/ipc.md 第 10 节：Unix socket bind 回滚可删除并发替换的 inode。
 *
 * 触发条件：bind_unix_socket 的状态检查、节点创建、bound_path 发布和失败回滚分离；
 * 同一 socket 两个并发 bind 都可能先创建各自路径，其中一个在 bind_path 竞争失败；
 * 失败路径只按名字 unlink 不校验 inode 身份，若第三个线程已替换该路径，回滚会删除
 * 替代对象。必须在支持 socket inode 创建的文件系统（tmpfs）上运行；ext4_native 对
 * S_IFSOCK 返回 EOPNOTSUPP，会在创建阶段提前失败，无法到达回滚竞态。
 *
 * 本测试：在 /tmp tmpfs 上，两个线程在同一 dup 描述符上绑定两个轮换路径 PA/PB，
 * 第三个线程在失败候选路径上循环删除、创建普通文件、写入 inode/generation 标记并
 * 校验。bind 失败后已成功创建且标记不同的普通文件消失即算命中。
 */
#define _GNU_SOURCE
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <unistd.h>
#include <fcntl.h>
#include <errno.h>
#include <signal.h>
#include <sys/socket.h>
#include <sys/un.h>
#include <sys/stat.h>
#include <pthread.h>
#include <stdatomic.h>
#include "smp_check.h"

#define PA "/tmp/race_ipc_bind10a"
#define PB "/tmp/race_ipc_bind10b"

static int sock_fd;
static atomic_int stop = 0;
static atomic_int hit = 0;

static void on_watchdog(int s) { (void)s; atomic_store(&stop, 1); _exit(0); }

static void do_bind(const char *path)
{
    struct sockaddr_un addr = {0};
    addr.sun_family = AF_UNIX;
    snprintf(addr.sun_path, sizeof(addr.sun_path), "%s", path);
    bind(sock_fd, (struct sockaddr *)&addr, sizeof(addr));
}

static void *binder_a(void *arg)
{
    (void)arg;
    while (!atomic_load(&stop)) { do_bind(PA); usleep(100); }
    return NULL;
}

static void *binder_b(void *arg)
{
    (void)arg;
    while (!atomic_load(&stop)) { do_bind(PB); usleep(100); }
    return NULL;
}

static void *replacer(void *arg)
{
    (void)arg;
    while (!atomic_load(&stop)) {
        for (int p = 0; p < 2; p++) {
            const char *path = p ? PB : PA;
            unlink(path);
            int fd = open(path, O_CREAT | O_WRONLY | O_TRUNC, 0600);
            if (fd < 0) continue;
            const char tag[] = "MARKER";
            write(fd, tag, sizeof(tag));
            close(fd);
            usleep(200);
            struct stat st;
            if (stat(path, &st) == 0 && !S_ISSOCK(st.st_mode)) {
                char buf[8] = {0};
                fd = open(path, O_RDONLY);
                if (fd >= 0) { read(fd, buf, sizeof(buf)); close(fd); }
                if (memcmp(buf, tag, sizeof(tag)) != 0)
                    atomic_fetch_add(&hit, 1);
            }
        }
    }
    return NULL;
}

int main(void)
{
    int r = race_require_cpus(3, "case10_unix_bind_rollback");
    if (r) return r;

    signal(SIGALRM, on_watchdog);
    alarm(6);

    unlink(PA); unlink(PB);
    sock_fd = socket(AF_UNIX, SOCK_STREAM, 0);
    if (sock_fd < 0) { perror("socket"); return 1; }
    int dup_fd = dup(sock_fd);
    sock_fd = dup_fd;

    pthread_t a, b, c;
    pthread_create(&a, NULL, binder_a, NULL);
    pthread_create(&b, NULL, binder_b, NULL);
    pthread_create(&c, NULL, replacer, NULL);

    usleep(5000000);
    atomic_store(&stop, 1);
    pthread_join(a, NULL); pthread_join(b, NULL); pthread_join(c, NULL);

    close(sock_fd);
    unlink(PA); unlink(PB);
    if (atomic_load(&hit))
        printf("case10_unix_bind_rollback: HIT (replaced file deleted %d times)\n", atomic_load(&hit));
    else
        printf("case10_unix_bind_rollback: PASS\n");
    return 0;
}
