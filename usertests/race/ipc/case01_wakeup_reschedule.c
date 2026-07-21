/*
 * 对应 docs/race/ipc.md 第 1 节：IPC 等待任务在切出前被重新调度。
 *
 * 触发条件：>=3 CPU 且未关闭 CONFIG_NO_SMP。等待者在 wait_current 改为 Blocked
 * 并入队后、调用 schedule() 切出前，另一 CPU 可通过 wakeup_task 把同一 TCB 改
 * Ready 并放入运行队列，第三个 CPU 可能在原 CPU 尚未切出时运行同一 TCB。
 *
 * 本测试：pipe 上多生产者/多消费者高频切换，每条消息携带单调序号。检查序号
 * 重复、回退、数据损坏；watchdog 超时判定长期无进展。命中：出现重复序号、数据
 * 校验失败或内核 panic。当前 UP/no-smp 配置下不会触发，但仍应稳定通过。
 */
#define _GNU_SOURCE
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <unistd.h>
#include <errno.h>
#include <signal.h>
#include <pthread.h>
#include <stdatomic.h>
#include "smp_check.h"

#define MSGS       4000
#define PRODUCERS  2
#define CONSUMERS  2
#define PAYLOAD    64

struct msg {
    int seq;
    int producer;
    char pad[PAYLOAD - sizeof(int) * 2];
};

static int pfd[2];
static atomic_int next_seq = 1;
static atomic_int seen = 0;
static atomic_int dup_cnt = 0;
static atomic_int bad = 0;
static atomic_int *got;

static void on_alarm(int s) { (void)s; _exit(2); }

static void *producer(void *arg)
{
    int id = (int)(long)arg;
    struct msg m;
    for (int i = 0; i < MSGS; i++) {
        m.seq = atomic_fetch_add(&next_seq, 1);
        m.producer = id;
        memset(m.pad, (char)(m.seq & 0xff), sizeof(m.pad));
        ssize_t n;
        do {
            n = write(pfd[1], &m, sizeof(m));
        } while (n < 0 && errno == EINTR);
        if (n != (ssize_t)sizeof(m)) {
            perror("write");
            _exit(3);
        }
    }
    return NULL;
}

static void *consumer(void *arg)
{
    (void)arg;
    struct msg m;
    for (;;) {
        ssize_t n;
        do {
            n = read(pfd[0], &m, sizeof(m));
        } while (n < 0 && errno == EINTR);
        if (n == 0) return NULL;
        if (n != (ssize_t)sizeof(m)) {
            atomic_fetch_add(&bad, 1);
            continue;
        }
        char exp = (char)(m.seq & 0xff);
        for (size_t k = 0; k < sizeof(m.pad); k++)
            if (m.pad[k] != exp) { atomic_fetch_add(&bad, 1); break; }
        int prev = got[m.seq];
        if (prev == 1) atomic_fetch_add(&dup_cnt, 1);
        got[m.seq] = 1;
        atomic_fetch_add(&seen, 1);
    }
}

int main(void)
{
    int r = race_require_cpus(3, "case01_wakeup_reschedule");
    if (r) return r;

    signal(SIGALRM, on_alarm);
    alarm(30);

    if (pipe(pfd) < 0) { perror("pipe"); return 1; }
    int total = PRODUCERS * MSGS;
    got = calloc(total + 1, sizeof(*got));
    if (!got) { perror("calloc"); return 1; }

    pthread_t pt[PRODUCERS], ct[CONSUMERS];
    for (int i = 0; i < PRODUCERS; i++)
        if (pthread_create(&pt[i], NULL, producer, (void *)(long)i)) { perror("pthread"); return 1; }
    for (int i = 0; i < CONSUMERS; i++)
        if (pthread_create(&ct[i], NULL, consumer, NULL)) { perror("pthread"); return 1; }

    for (int i = 0; i < PRODUCERS; i++) pthread_join(pt[i], NULL);
    close(pfd[1]);
    for (int i = 0; i < CONSUMERS; i++) pthread_join(ct[i], NULL);

    int d = atomic_load(&dup_cnt), b = atomic_load(&bad), s = atomic_load(&seen);
    if (s != total) {
        fprintf(stderr, "FAIL: seen=%d expected=%d dup=%d bad=%d\n", s, total, d, b);
        return 1;
    }
    if (d || b) {
        fprintf(stderr, "FAIL: dup=%d bad=%d\n", d, b);
        return 1;
    }
    printf("case01_wakeup_reschedule: PASS\n");
    return 0;
}
