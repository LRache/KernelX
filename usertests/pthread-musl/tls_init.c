#include <pthread.h>
#include <stdint.h>
#include <stdio.h>

__thread int tls_fix = 23;
__thread int tls_zero;

static int check_tls(void)
{
    int failed = 0;

    if (tls_fix != 23) {
        printf("fixed init failed: want 23 got %d\n", tls_fix);
        failed = 1;
    }
    if (tls_zero != 0) {
        printf("zero init failed: want 0 got %d\n", tls_zero);
        failed = 1;
    }

    return failed;
}

static void *thread_main(void *arg)
{
    (void)arg;

    int failed = check_tls();

    tls_fix++;
    tls_zero++;

    return (void *)(uintptr_t)failed;
}

#define ARRAY_LEN(a) (sizeof(a) / sizeof((a)[0]))

int main(void)
{
    pthread_t threads[5];
    int failed = check_tls();

    for (size_t j = 0; j < 2; j++) {
        size_t created = 0;

        for (size_t i = 0; i < ARRAY_LEN(threads); i++) {
            int ret = pthread_create(&threads[i], NULL, thread_main, NULL);
            if (ret != 0) {
                printf("pthread_create failed: %d\n", ret);
                failed = 1;
                break;
            }

            created++;
            tls_fix++;
            tls_zero++;
        }

        for (size_t i = 0; i < created; i++) {
            void *result = NULL;
            int ret = pthread_join(threads[i], &result);
            if (ret != 0) {
                printf("pthread_join failed: %d\n", ret);
                failed = 1;
            }
            if (result != NULL) {
                printf("thread tls check failed: %zu\n", i);
                failed = 1;
            }
        }
    }

    if (failed) {
        return 1;
    }

    puts("tls_init: PASS");
    return 0;
}
