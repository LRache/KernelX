#include <stdio.h>
#include <linux/sched.h>
#include <unistd.h>
#include <sys/wait.h>

int main() {
    printf("Hello, World!\n");
    int pid = fork();
    if (pid < 0) {
        perror("fork failed");
        return 1;
    } else if (pid == 0) {
        printf("Child process created with PID: %d\n", getpid());
    } else {
        // waitpid(pid, NULL, 0);
        printf("Parent process with PID: %d created child with PID: %d\n",
               getpid(), pid);
        waitpid(-1, NULL, 0);
    }

    return 0;
}