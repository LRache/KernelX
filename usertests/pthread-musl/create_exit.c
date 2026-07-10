#include <pthread.h>
#include <stdio.h>

struct worker_args {
    int input;
    int output;
};

static void *worker_main(void *arg)
{
    struct worker_args *args = arg;

    args->output = args->input + 1;
    pthread_exit(&args->output);

    return NULL;
}

int main(void)
{
    pthread_t thread;
    struct worker_args args = {
        .input = 41,
        .output = 0,
    };
    void *result = NULL;
    int ret;

    ret = pthread_create(&thread, NULL, worker_main, &args);
    if (ret != 0) {
        printf("pthread_create failed: %d\n", ret);
        return 1;
    }

    ret = pthread_join(thread, &result);
    if (ret != 0) {
        printf("pthread_join failed: %d\n", ret);
        return 1;
    }

    if (result != &args.output || args.output != 42) {
        printf("pthread result mismatch: result=%p output=%d\n", result, args.output);
        return 1;
    }

    puts("pthread_create_exit: PASS");
    return 0;
}
