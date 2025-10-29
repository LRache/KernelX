#include <stdint.h>
#include <stdio.h>
#include <sys/syscall.h>
#include <unistd.h>
#include <sys/time.h>
#include <syscall.h>

unsigned long long get_us() {
    struct timeval tv;
    syscall(SYS_gettimeofday, &tv, NULL);
    // gettimeofday(&tv, NULL);
    return (unsigned long long)tv.tv_sec * 1000000 + tv.tv_usec;
}

void loop_sleep(unsigned long long us) {
    unsigned long long end = get_us() + us;
    unsigned long long now;
    while ((now = get_us()) < end) {
        // printf("now = %llu, end = %llu\n", now, end);
    }
}

int main() {
    printf("Test loopsleep\n");

    unsigned int i = 0;
    while (1) {
        loop_sleep(1000000); // sleep for 1 second
        i ++;
        printf("%u second passed\n", i);
        fflush(stdout);
    }

    return 0;
}
