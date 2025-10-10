#include <stdio.h>
#include <signal.h>

int main() {
    struct sigaction sa;
    sa.sa_handler = SIG_IGN; // Ignore the signal
    sigemptyset(&sa.sa_mask);
    sa.sa_flags = 0;

    if (sigaction(SIGINT, &sa, NULL) == -1) {
        perror("sigaction");
        return 1;
    }

    printf("SIGINT is now ignored. Press Ctrl+C to test.\n");
    // Wait for a while to allow testing
    sleep(10);
    printf("Exiting after 10 seconds.\n");

    return 0;
}
