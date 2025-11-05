#include <pthread.h>
#include <semaphore.h>
#include <string.h>

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
	int r;
	void *res;

	sem_init(&sem1, 0, 0);

	/* Asynchronous cancellation */
	TESTR(r, pthread_create(&td, 0, start_async, &sem1), "failed to create thread");
	while (sem_wait(&sem1));
	TESTR(r, pthread_cancel(td), "canceling");
	TESTR(r, pthread_join(td, &res), "joining canceled thread");
	TESTC(res == PTHREAD_CANCELED, "canceled thread exit status");

    return 0;
}