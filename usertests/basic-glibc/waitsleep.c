#include <stdio.h>
#include <time.h>
#include <unistd.h>
#include <sys/wait.h>

int main() {
    pid_t pid = fork();
    if (pid == -1) {
        perror("fork");
        return 1;
    }

    if (pid == 0) {
        // Child process
        printf("Child sleeping for 1 second...\n");
        fflush(stdout);
        
        struct timespec req = {1, 0}; // 1 second, 0 nanoseconds
        int r = nanosleep(&req, NULL);
        if (r != 0) {
            perror("sleep");
            return 1;
        }
        
        printf("Child awake!\n");
    } else {
        // Parent process
        waitpid(pid, NULL, 0); // Wait for child to finish
        printf("Parent: Child has finished execution.\n");
    }

    return 0;
}
