/*
 * 对应 docs/race/ipc.md 第 6 节：Unix socket 双向 poll 遗留反方向等待项。
 *
 * 触发条件：Unix socket 同时等 POLLIN|POLLOUT 时分别在 rx 和 tx 注册同一 TCB；
 * 一个方向唤醒后只清该方向队列，poll/select 对获胜文件跳过 wait_event_cancel，
 * 使另一方向等待项残留，之后可在 TCB 阻塞于无关操作时用旧 Event::Poll 错误唤醒。
 *
 * 本测试（SOCK_STREAM socketpair）：A→B 发送队列填满、B→A 接收队列为空；T 对 A 同时
 * 等 POLLIN|POLLOUT；B 向 A 写数据使 rx 唤醒返回 POLLIN，A 的 tx 等待项未删；T 随后
 * 阻塞读另一个空 pipe；B 再读 A 之前发送数据使 A 的 tx 可写，残留等待项唤醒 T。检查
 * 无关 pipe 在无数据时提前返回、错误返回或内核 panic。注：本内核 socket 系统调用走
 * InetSocket 对 Unix socket 返回 ENOTSOCK，必须用 read/write 而非 send/recv。
 */
#define _GNU_SOURCE
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <unistd.h>
#include <errno.h>
#include <signal.h>
#include <sys/socket.h>
#include <poll.h>
#include <pthread.h>
#include <stdatomic.h>
#include "smp_check.h"

static int sp[2];
static int pipefd[2];
static atomic_int phase = 0;
static atomic_int spurious = 0;

static void on_watchdog(int s) { (void)s; _exit(2); }

static void fill(int fd)
{
    char buf[256];
    memset(buf, 'x', sizeof(buf));
    for (;;) {
        int n = write(fd, buf, sizeof(buf));
        if (n < 0 && errno == EINTR) continue;
        if (n <= 0) break;
    }
}

static void *peer(void *arg)
{
    (void)arg;
    while (atomic_load(&phase) < 1) usleep(100);
    fill(sp[1]);

    atomic_store(&phase, 2);
    usleep(50 * 1000);

    write(sp[1], "Y", 1);
    while (atomic_load(&phase) < 3) usleep(100);

    char buf[256];
    for (;;) {
        int n = read(sp[0], buf, sizeof(buf));
        if (n <= 0) { if (n < 0 && errno == EINTR) continue; break; }
    }
    atomic_store(&phase, 4);
    return NULL;
}

int main(void)
{
    int rc = race_require_cpus(2, "case06_unix_poll_leftover");
    if (rc) return rc;

    signal(SIGALRM, on_watchdog);
    alarm(10);

    if (socketpair(AF_UNIX, SOCK_STREAM, 0, sp) < 0) { perror("socketpair"); return 1; }
    if (pipe(pipefd) < 0) { perror("pipe"); return 1; }

    pthread_t p;
    pthread_create(&p, NULL, peer, NULL);

    atomic_store(&phase, 1);
    while (atomic_load(&phase) < 2) usleep(100);

    struct pollfd pf = { .fd = sp[0], .events = POLLIN | POLLOUT };
    int r = poll(&pf, 1, 2000);
    if (r <= 0) { fprintf(stderr, "poll timeout/error %d\n", r); return 1; }

    if (pf.revents & POLLIN) {
        char c; read(sp[0], &c, 1);
    }

    atomic_store(&phase, 3);

    struct pollfd pp = { .fd = pipefd[0], .events = POLLIN };
    r = poll(&pp, 1, 800);
    if (r > 0 && !(pp.revents & (POLLIN | POLLERR | POLLHUP))) {
        atomic_fetch_add(&spurious, 1);
    }
    if (r > 0 && (pp.revents & POLLIN)) {
        atomic_fetch_add(&spurious, 1);
    }

    while (atomic_load(&phase) < 4) usleep(100);

    close(sp[0]); close(sp[1]); close(pipefd[0]); close(pipefd[1]);
    pthread_join(p, NULL);

    if (atomic_load(&spurious))
        printf("case06_unix_poll_leftover: HIT (spurious wakeup on unrelated pipe)\n");
    else
        printf("case06_unix_poll_leftover: PASS\n");
    return 0;
}
