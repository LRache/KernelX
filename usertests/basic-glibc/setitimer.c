#include <stdio.h>
#include <signal.h>
#include <sys/time.h>
#include <unistd.h>

void timer_handler(int _) {
    static int count = 0;
    printf("Timer expired %d times\n", ++count);
    fflush(stdout);

    if (count >= 5) {
        struct itimerval timer;
        timer.it_value.tv_sec = 0;
        timer.it_value.tv_usec = 0;
        timer.it_interval.tv_sec = 0;
        timer.it_interval.tv_usec = 0;

        printf("Exiting after 5 timer expirations.\n");
        setitimer(ITIMER_REAL, &timer, NULL); // Disable the timer
    }
}

int main() {
    struct itimerval timer;

    signal(SIGALRM, timer_handler);

    timer.it_value.tv_sec = 1;
    timer.it_value.tv_usec = 0;

    timer.it_interval.tv_sec = 0;
    timer.it_interval.tv_usec = 500000;
    // timer.it_interval.tv_usec = 0;

    if (setitimer(ITIMER_REAL, &timer, NULL) == -1) {
        perror("Error calling setitimer");
        return 1;
    }

    while (1) {
        pause();
    }

    return 0;
}