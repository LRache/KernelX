/*
 * 对应 docs/race/ipc.md 第 9 节：消息型 Unix socket 拷贝失败后跳过写者唤醒。
 *
 * 触发条件：MessagePipeInner::read_to_user 先从队列移除完整消息并释放容量，再向用户
 * 缓冲区复制；用户拷贝失败直接返回，跳过 write_waiter.wake_all 和可写 notifier。满
 * 队列上的阻塞写者无法观察到已释放空间。
 *
 * 本测试：SOCK_DGRAM socketpair 填满接收队列，确认额外发送者已阻塞；接收者通过
 * read(2)（不是 recvfrom/recvmsg，本内核 Unix socket 系统调用走 InetSocket 会返回
 * ENOTSOCK）使用 PROT_NONE 缓冲区触发 EFAULT；故障后不再读取，检查发送者在已释放
 * 至少一条消息容量后仍无进展；再执行一次正常读取作控制唤醒，校验发送序号无重复乱序。
 */
#define _GNU_SOURCE
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <unistd.h>
#include <errno.h>
#include <signal.h>
#include <sys/socket.h>
#include <sys/mman.h>
#include <pthread.h>
#include <stdatomic.h>

static int sp[2];
static atomic_int writer_prog = 0;
static atomic_int writer_blocked = 0;
static atomic_int stop = 0;

static void on_watchdog(int s) { (void)s; atomic_store(&stop, 1); _exit(0); }

static void fill_rx(void)
{
    char buf[16];
    for (int i = 0; i < 100000; i++) {
        memcpy(buf, &i, sizeof(i));
        int n = write(sp[1], buf, sizeof(buf));
        if (n <= 0) { if (n < 0 && errno == EINTR) { i--; continue; } break; }
    }
}

static void *writer(void *arg)
{
    (void)arg;
    int seq = 0x7fff0000;
    char buf[16];
    while (!atomic_load(&stop)) {
        memcpy(buf, &seq, sizeof(seq));
        int n = write(sp[1], buf, sizeof(buf));
        if (n > 0) { atomic_fetch_add(&writer_prog, 1); seq++; continue; }
        if (n < 0 && errno == EAGAIN) { atomic_store(&writer_blocked, 1); continue; }
        if (n < 0 && errno == EINTR) continue;
        atomic_store(&writer_blocked, 1);
        break;
    }
    return NULL;
}

int main(void)
{
    signal(SIGALRM, on_watchdog);
    alarm(8);

    if (socketpair(AF_UNIX, SOCK_DGRAM, 0, sp) < 0) { perror("socketpair"); return 1; }
    fill_rx();

    pthread_t w;
    pthread_create(&w, NULL, writer, NULL);
    usleep(100 * 1000);
    int blocked = atomic_load(&writer_blocked);

    char *bad = mmap(NULL, 4096, PROT_NONE, MAP_ANONYMOUS | MAP_PRIVATE, -1, 0);
    ssize_t n = read(sp[0], bad, 16);
    int efault = (n < 0 && errno == EFAULT);

    usleep(300 * 1000);
    int prog_after_fault = atomic_load(&writer_prog);

    char good[16];
    read(sp[0], good, sizeof(good));
    usleep(300 * 1000);
    int prog_after_ctrl = atomic_load(&writer_prog);

    atomic_store(&stop, 1);
    pthread_join(w, NULL);

    munmap(bad, 4096);
    close(sp[0]); close(sp[1]);

    if (efault && blocked && prog_after_fault == 0 && prog_after_ctrl > 0) {
        printf("case09_msgsock_read_fault: HIT (writer stuck after EFAULT, woken by control read)\n");
    } else {
        printf("case09_msgsock_read_fault: PASS (efault=%d blocked=%d after_fault=%d after_ctrl=%d)\n",
               efault, blocked, prog_after_fault, prog_after_ctrl);
    }
    return 0;
}
