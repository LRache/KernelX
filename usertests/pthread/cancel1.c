#include <errno.h>
#include <pthread.h>
#include <semaphore.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

static sem_t sem_seq;      // 用来卡住被测线程，确保先投递 cancel 再放行
static sem_t sem_test;     // 辅助线程用：初值为 1，让辅助线程很快退出，从而 join 基本不阻塞
static pthread_t td_aux;   // 被 join 的目标线程
static volatile int seqno; // 1: 进入被测调用前；2: 被测调用返回后

#define TRY0(x) do{ int _r = (x); if(_r){ \
  fprintf(stderr, "%s failed: %s\n", #x, strerror(_r)); exit(2);} }while(0)
#define TRYM1(x) do{ if((x)==-1){ \
  fprintf(stderr, "%s failed: %s\n", #x, strerror(errno)); exit(2);} }while(0)

static void* aux_run(void* arg) {
  // 初值 1：这里不会阻塞，很快退出，制造“non-blocking join”
  // 用 while(..); 的写法避免被 EINTR 打断
  while (sem_wait(&sem_test)) {}
  return NULL;
}

static void* tested_run(void* arg) {
  // 1) 禁用取消，避免在卡点（sem_seq）被取消
  pthread_setcancelstate(PTHREAD_CANCEL_DISABLE, NULL);

  // 2) 卡在这里，等待主线程：先 cancel 再放行
  while (sem_wait(&sem_seq)) {}

  // 3) 启用取消，准备进入被测调用
  pthread_setcancelstate(PTHREAD_CANCEL_ENABLE, NULL);

  seqno = 1; // 马上要进入取消点

  // 4) 被测调用：non-blocking pthread_join（目标线程已基本退出）
  //    由于已有挂起的取消请求，进入这里就应当被取消，通常不会返回到下一行
  TRY0(pthread_join(td_aux, NULL));

  seqno = 2; // 若能走到这里，说明没在入口被取消（测试失败）
  return NULL;
}

int main(void) {
  pthread_t td_tested;
  void* res = NULL;

  TRY0(sem_init(&sem_seq, 0, 0));
  TRY0(sem_init(&sem_test, 0, 1)); // 关键：设为 1，制造“non-blocking join”

  // 创建“很快结束”的目标线程
  TRY0(pthread_create(&td_aux, NULL, aux_run, NULL));

  // 创建被测线程（将在 pthread_join 处命中取消点）
  TRY0(pthread_create(&td_tested, NULL, tested_run, NULL));

  // 先投递取消，再放行到被测调用
  TRY0(pthread_cancel(td_tested));
  TRY0(sem_post(&sem_seq));

  // 等待被测线程结束，并检查返回值是否为 PTHREAD_CANCELED
  TRY0(pthread_join(td_tested, &res));

  // 收尾：被测线程没有 join 辅助线程，因此主线程来收尸
  TRY0(pthread_join(td_aux, NULL));

  // 销毁信号量
  TRY0(sem_destroy(&sem_seq));
  TRY0(sem_destroy(&sem_test));

  int ok = (res == PTHREAD_CANCELED) && (seqno == 1);
  printf("[non-blocking pthread_join] %s\n",
         ok ? "PASS: 进入 pthread_join 即被取消 (seqno==1)"
            : "FAIL: 未在入口被取消 或 seqno!=1");

  return ok ? 0 : 1;
}
