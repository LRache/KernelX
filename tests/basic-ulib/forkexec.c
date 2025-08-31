#include <stdio.h>
#include <unistd.h>
#include <sys/wait.h>

int main() {
    int pid = fork();
    if (pid == 0) {
        execve("/basic-ulib/forkexec-child", NULL, NULL);
        puts("Failed to execute child process!");
        return 0;
    } else {
        puts("Hello from parent process!");
    }

    wait4(0, NULL, 0, NULL);
    wait4(0, NULL, 0, NULL);

    return 0;
}