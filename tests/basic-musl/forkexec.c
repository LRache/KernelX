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
    } 
    
    if (pid == 0) {
        char *args[] = {"/basic-musl/forkexec-child", "argv[1]", "argv[2]", NULL};
        char *envp[] = {"env1=var1", "env2=var2", NULL};
        execve("/basic-musl/forkexec-child", args, envp);
    } else {
        waitpid(pid, NULL, 0);
        printf("Parent process with PID: %d created child with PID: %d\n",
               getpid(), pid);
    }
    return 0;
}