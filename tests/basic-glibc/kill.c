#include <stdio.h>
#include <stdlib.h>
#include <unistd.h>
#include <signal.h>
#include <sys/wait.h>

int main() {
    pid_t pid;
    pid = fork();
    
    if (pid < 0) {
        perror("fork failed");
        exit(1);
    } 
    
    if (pid == 0) {
        printf("Child process (PID: %d) running dead loop...\n", getpid());
        fflush(stdout);
        while (1);
    } 
    
    // Parent process
    sleep(1); // Ensure child is running
    printf("Parent process (PID: %d) sending SIGKILL to child (PID: %d)...\n", getpid(), pid);
    if (kill(pid, SIGKILL) == -1) {
        perror("kill failed");
        exit(1);
    }
    wait(NULL); // Wait for child to terminate

    pid = fork();
    if (pid < 0) {
        perror("fork failed");
        exit(1);
    }

    if (pid == 0) {
        printf("Child process (PID: %d) sleeping for 5 seconds...\n", getpid());
        fflush(stdout);
        sleep(5);
        printf("SHOULD NOT REACH HERE!\n");
        return -1;
    }

    // Parent process
    sleep(1); // Ensure child is sleeping
    printf("Parent process (PID: %d) sending SIGKILL to child (PID: %d)...\n", getpid(), pid);
    if (kill(pid, SIGKILL) == -1) {
        perror("kill failed");
        exit(1);
    }
    wait(NULL); // Wait for child to terminate
    return 0;
}
