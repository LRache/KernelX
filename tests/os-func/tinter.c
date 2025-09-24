#include <stdint.h>
#include <stdio.h>
#include <unistd.h>
#include <sys/time.h>

unsigned long long get_us() {
    struct timeval tv;
    gettimeofday(&tv, NULL);
    return (unsigned long long)tv.tv_sec * 1000000 + tv.tv_usec;
}

int main() {
    printf("Test tinter\n\n\n");

    // int pid = fork();
    // if (pid < 0) {
    //     perror("fork");
    //     return 1;
    // }

    // if (pid == 0) {
    //     execve("./tinter-sub", (char *const[]){"./tinter-sub", NULL}, (char *const[]){NULL});
    //     perror("execve");
    //     return 1;
    // }

    while (1) {
        unsigned long long start = get_us();
        while (1) {
            volatile unsigned long long now, diff;
            while ((diff = (now = get_us()) - start) < 100000) // busy wait for 1s
                printf("diff = %llu\n", diff);
            printf("1 second passed in parent process\n");
            start = get_us();
        }
    }

    return 0;
}
