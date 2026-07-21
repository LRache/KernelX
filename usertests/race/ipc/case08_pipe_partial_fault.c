/*
 * 对应 docs/race/ipc.md 第 8 节：pipe 用户拷贝部分提交后跳过读者唤醒。
 *
 * 触发条件：FIFO::push_back_ubuf 逐页把数据写入 FIFO，前面页已增加 FIFO 长度后，
 * 后续页翻译失败通过 ? 直接返回 EFAULT，绕过 read_waiter.wake_all 和 epoll notifier。
 * FIFO 已可读但已阻塞读者未收到通知。
 *
 * 本测试：读者先稳定阻塞在空 pipe；写者用跨“有效页/保护页”边界的缓冲区，前缀有效、
 * 后一页 PROT_NONE。写预期返回 EFAULT；不二次写、不关闭写端，watchdog 判断读者能否
 * 读到已提交前缀；之后再写一字节作控制唤醒，校验读者读到的数据含此前应通知的前缀。
 * 读者在控制唤醒前就读到前缀则 PASS（唤醒未被跳过）；控制唤醒后才读到且前缀存在则
 * 命中（部分提交被跳过唤醒）。
 */
#define _GNU_SOURCE
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <unistd.h>
#include <errno.h>
#include <signal.h>
#include <sys/mman.h>
#include <pthread.h>
#include <stdatomic.h>

#define PAGE 4096
static int pfd[2];
static atomic_int reader_got = 0;
static atomic_int reader_prefix = 0;

static void on_watchdog(int s)
{
    (void)s;
    if (!atomic_load(&reader_got)) {
        printf("case08_pipe_partial_fault: HIT (reader stuck after partial commit)\n");
    }
    _exit(0);
}

static void *reader(void *arg)
{
    (void)arg;
    char buf[PAGE * 2];
    ssize_t n = read(pfd[0], buf, sizeof(buf));
    if (n > 0) {
        atomic_store(&reader_got, (int)n);
        if (buf[0] == 'A') atomic_store(&reader_prefix, 1);
    }
    return NULL;
}

int main(void)
{
    signal(SIGALRM, on_watchdog);
    alarm(5);

    if (pipe(pfd) < 0) { perror("pipe"); return 1; }

    char *pages = mmap(NULL, PAGE * 2, PROT_READ | PROT_WRITE,
                       MAP_ANONYMOUS | MAP_PRIVATE, -1, 0);
    if (pages == MAP_FAILED) { perror("mmap"); return 1; }
    memset(pages, 'A', PAGE);
    if (mprotect(pages + PAGE, PAGE, PROT_NONE) < 0) { perror("mprotect"); return 1; }

    pthread_t r;
    pthread_create(&r, NULL, reader, NULL);
    usleep(100 * 1000);

    ssize_t n = write(pfd[1], pages, PAGE * 2);
    int efault = (n < 0 && errno == EFAULT);

    usleep(800 * 1000);
    int got_before = atomic_load(&reader_got);

    char ctrl = 'B';
    write(pfd[1], &ctrl, 1);
    pthread_join(r, NULL);

    munmap(pages, PAGE * 2);
    close(pfd[0]); close(pfd[1]);

    if (efault && got_before > 0) {
        printf("case08_pipe_partial_fault: PASS (reader notified despite EFAULT)\n");
        return 0;
    }
    if (efault && atomic_load(&reader_prefix) && got_before == 0) {
        printf("case08_pipe_partial_fault: HIT (wakeup skipped after partial commit)\n");
        return 0;
    }
    printf("case08_pipe_partial_fault: PASS (efault=%d got=%d)\n", efault, atomic_load(&reader_got));
    return 0;
}
