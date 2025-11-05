#include <pthread.h>
#include <time.h>

static void *start_signal(void *arg)
{
	void **args = arg;
	pthread_mutex_lock(args[1]);
	pthread_cond_signal(args[0]);
	pthread_mutex_unlock(args[1]);
	return 0;
}

static void *start_wait(void *arg)
{
	void **args = arg;
	pthread_mutex_t *m = args[1];
	pthread_cond_t *c = args[0];
	int *x = args[2];

	pthread_mutex_lock(m);
	while (*x) pthread_cond_wait(c, m);
	pthread_mutex_unlock(m);

	return 0;
}

int main(void)
{
	pthread_t td, td1, td2, td3;
	void *res;
	pthread_mutex_t mtx;
	pthread_cond_t cond;
	int foo[1];

	/* Condition variables */
	pthread_mutex_init(&mtx, 0);
	pthread_cond_init(&cond, 0);
	pthread_mutex_lock(&mtx);
	pthread_create(&td, 0, start_signal, (void *[]){ &cond, &mtx });
	pthread_cond_wait(&cond, &mtx);
	pthread_join(td, &res);
	// pthread_mutex_unlock(&mtx);
	// pthread_mutex_destroy(&mtx);
	// pthread_cond_destroy(&cond);

	/* Condition variables with multiple waiters */
	// pthread_mutex_init(&mtx, 0);
	// pthread_cond_init(&cond, 0);
	// pthread_mutex_lock(&mtx);
	// foo[0] = 1;
	// pthread_create(&td1, 0, start_wait, (void *[]){ &cond, &mtx, foo });
	// pthread_create(&td2, 0, start_wait, (void *[]){ &cond, &mtx, foo });
	// pthread_create(&td3, 0, start_wait, (void *[]){ &cond, &mtx, foo });
	// pthread_mutex_unlock(&mtx);
	// nanosleep(&(struct timespec){.tv_nsec=1000000}, 0);
	// foo[0] = 0;
	// pthread_mutex_lock(&mtx);
	// pthread_cond_signal(&cond);
	// pthread_mutex_unlock(&mtx);
	// pthread_mutex_lock(&mtx);
	// pthread_cond_signal(&cond);
	// pthread_mutex_unlock(&mtx);
	// pthread_mutex_lock(&mtx);
	// pthread_cond_signal(&cond);
	// pthread_mutex_unlock(&mtx);
	// pthread_join(td1, 0);
	// pthread_join(td2, 0);
	// pthread_join(td3, 0);
	// pthread_mutex_destroy(&mtx);
	// pthread_cond_destroy(&cond);

	/* Condition variables with broadcast signals */
	// pthread_mutex_init(&mtx, 0);
	// pthread_cond_init(&cond, 0);
	// pthread_mutex_lock(&mtx);
	// foo[0] = 1;
	// pthread_create(&td1, 0, start_wait, (void *[]){ &cond, &mtx, foo });
	// pthread_create(&td2, 0, start_wait, (void *[]){ &cond, &mtx, foo });
	// pthread_create(&td3, 0, start_wait, (void *[]){ &cond, &mtx, foo });
	// pthread_mutex_unlock(&mtx);
	// nanosleep(&(struct timespec){.tv_nsec=1000000}, 0);
	// pthread_mutex_lock(&mtx);
	// foo[0] = 0;
	// pthread_mutex_unlock(&mtx);
	// pthread_cond_broadcast(&cond);
	// pthread_join(td1, 0);
	// pthread_join(td2, 0);
	// pthread_join(td3, 0);
	// pthread_mutex_destroy(&mtx);
	// pthread_cond_destroy(&cond);

	return 0;
}
