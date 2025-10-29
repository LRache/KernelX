#include <stdio.h>
#include <stdlib.h>
#include <signal.h>
#include <unistd.h>
#include <time.h>
#include <sys/wait.h>

void sigchld_handler(int signum) {
    printf("[SIGCHLD Handler] Signal %d received. Child process changed state.\n", signum);
    // fflush(stdout);
}

int main() {    
    struct sigaction sa;
    sa.sa_handler = sigchld_handler;
    sigemptyset(&sa.sa_mask);
    sa.sa_flags = 0;
    
    if (sigaction(SIGCHLD, &sa, NULL) == -1) {
        perror("sigaction");
        return 0;
    }
    
    pid_t pid = fork();
    
    if (pid < 0) {
        perror("fork");
        return 0;
    }
    
    if (pid == 0) {
        struct timespec req = {1, 0}; // 1 second, 0 nanoseconds
        int r = nanosleep(&req, NULL);
        printf("[Child] Exited\n");
        exit(42);
    } else {
        printf("[Parent] Wait for PID=%d\n", pid);
        
        int status;
        pid_t result = wait(&status);
        
        if (result == -1) {
            perror("wait");
        } else {
            printf("[Parent] wait() success, child PID=%d\n", result);
        }
    }

    return 0;
}