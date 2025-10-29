#include <stdint.h>
#include <stdio.h>
#include <unistd.h>
#include <sys/time.h>
#include <syscall.h>

unsigned long long get_us() {
    struct timeval tv;
    syscall(SYS_gettimeofday, &tv, NULL);
    return (unsigned long long)tv.tv_sec * 1000000 + tv.tv_usec;
}

int main() {
    printf("Test tinter\n");
    fflush(stdout);

    int pid = fork();
    if (pid < 0) {
        perror("fork");
        return 1;
    }

    if (pid == 0) {
        printf("Child Process\n");
        fflush(stdout);
        while (1) {
            unsigned long long start = get_us();
            for (volatile unsigned int i = 0; i < 75000000; i++)
                ;
            unsigned long long end = get_us();
            printf("Child Loop takes %llu us\n", end - start);
            fflush(stdout);
        }
    } else {
        printf("Parent Process\n");
        fflush(stdout);
        while (1) {
            unsigned long long start = get_us();
            for (volatile unsigned int i = 0; i < 100000000; i++)
                ;
            unsigned long long end = get_us();
            printf("Parent Loop takes %llu us\n", end - start);
            fflush(stdout);
        }
    }

    

    return 0;
}
