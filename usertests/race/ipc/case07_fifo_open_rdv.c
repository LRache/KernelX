/*
 * 对应 docs/race/ipc.md 第 7 节：FIFO open 的短暂对端会合被快速关闭抹除。
 *
 * 触发条件：阻塞打开 FIFO 时本端先增 reader/writer 计数再等对端；等待循环只检查
 * 当前对端计数，不记录“对端曾成功打开并完成会合”。对端在唤醒等待者后快速关闭，
 * 等待者再次检查看到计数为 0 又睡眠，尽管对端 open 已成功返回。
 *
 * 本测试：读等待写、写等待读两方向分别测试。一个进程持续执行长等待单向阻塞打开，
 * 多个对端进程执行“一次打开成功后立即关闭”。watchdog 限定时间内对应等待打开未返回
 * 即算命中。使用 /tmp tmpfs（内核 ext4 不把 FIFO 当 pipe 打开）。
 */
#define _GNU_SOURCE
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <unistd.h>
#include <fcntl.h>
#include <errno.h>
#include <signal.h>
#include <sys/stat.h>
#include <sys/wait.h>
#include "smp_check.h"

#define FIFO "/tmp/race_ipc_fifo7"
#define ROUNDS 200

static volatile sig_atomic_t timed_out = 0;
static void on_alarm(int s) { (void)s; timed_out = 1; _exit(0); }

static int wait_open(int flags, int timeout_ms)
{
    signal(SIGALRM, on_alarm);
    ualarm(timeout_ms * 1000, 0);
    int fd = open(FIFO, flags);
    ualarm(0, 0);
    return fd;
}

int main(void)
{
    int r = race_require_cpus(2, "case07_fifo_open_rdv");
    if (r) return r;

    unlink(FIFO);
    if (mkfifo(FIFO, 0600) < 0) { perror("mkfifo"); return 1; }

    int stuck = 0;
    for (int i = 0; i < ROUNDS; i++) {
        int dir = i % 2;
        pid_t peer_pid = fork();
        if (peer_pid == 0) {
            int pfd = open(FIFO, dir ? O_RDONLY : O_WRONLY);
            if (pfd >= 0) close(pfd);
            _exit(0);
        }
        usleep(2000);
        int fd = wait_open(dir ? O_WRONLY : O_RDONLY, 500);
        if (fd < 0 && timed_out) { stuck++; timed_out = 0; }
        else if (fd >= 0) close(fd);
        int st; waitpid(peer_pid, &st, 0);
    }

    unlink(FIFO);
    if (stuck)
        printf("case07_fifo_open_rdv: HIT (stuck %d/%d)\n", stuck, ROUNDS);
    else
        printf("case07_fifo_open_rdv: PASS\n");
    return 0;
}
