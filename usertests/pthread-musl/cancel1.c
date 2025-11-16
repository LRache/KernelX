#include <pthread.h>
#include <semaphore.h>

static void *start_async(void *arg)
{
    pthread_setcanceltype(PTHREAD_CANCEL_ASYNCHRONOUS, 0);
    sem_post(arg);
    for (;;);
    return 0;
}

int main(void)
{
    pthread_t td;
    sem_t sem1;
    void *res;

    sem_init(&sem1, 0, 0);

    /* Asynchronous cancellation */
    pthread_create(&td, 0, start_async, &sem1);
    while (sem_wait(&sem1));
    pthread_cancel(td);
    pthread_join(td, &res);

    return 0;
}