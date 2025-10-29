#include <sys/wait.h>
#include <unistd.h>

int main() {
    pid_t pid = fork();
    if (pid == 0) {
        char *const args[] = {"/basic-ulib/args-child", "args[1]", "args[2]", NULL};
        char *const envp[] = {"ENV_VAR1=value1", "ENV_VAR2=value2", NULL};
        execve(args[0], args, envp);
    }

    wait4(pid, NULL, 0, NULL);

    return 0;
}
