#include <stdio.h>
#include <unistd.h>
#include <sys/wait.h>

// vfork shares the address space, so this variable will be shared.
volatile int shared_var = 100;

int main() {
    printf("Parent [PID: %d]: initial shared_var = %d\n", getpid(), shared_var);
    fflush(stdout);

    pid_t pid = vfork();

    if (pid < 0) {
        // Error
        perror("vfork failed");
        return 1;
    } else if (pid == 0) {
        // Child process
        printf("Child [PID: %d]: executing.\n", getpid());
        fflush(stdout);
        printf("Child [PID: %d]: original shared_var = %d\n", getpid(), shared_var);
        fflush(stdout);
        
        // Modify the shared variable. The parent will see this change.
        shared_var = 200;
        printf("Child [PID: %d]: modified shared_var to %d\n", getpid(), shared_var);
        fflush(stdout);
        
        printf("Child [PID: %d]: exiting.\n", getpid());
        fflush(stdout);
        // Use _exit() with vfork to prevent flushing parent's stdio buffers
        // and other process-level cleanup that would affect the parent.
        _exit(0);
    } else {
        // Parent process
        // The parent is suspended until the child calls execve() or _exit().
        printf("Parent [PID: %d]: resumed.\n", getpid());
        fflush(stdout);
        printf("Parent [PID: %d]: shared_var is now %d\n", getpid(), shared_var);
        fflush(stdout);

        if (shared_var == 200) {
            printf("Parent [PID: %d]: Success! The variable was modified by the child.\n", getpid());
            fflush(stdout);
        } else {
            printf("Parent [PID: %d]: Failure! The variable was not modified by the child.\n", getpid());
            fflush(stdout);
        }

        // Wait for the child to ensure it's reaped.
        int status;
        waitpid(pid, &status, 0);
        if (WIFEXITED(status) && WEXITSTATUS(status) == 0) {
            printf("Parent [PID: %d]: Child terminated successfully.\n", getpid());
            fflush(stdout);
        } else {
            printf("Parent [PID: %d]: Child terminated with an error.\n", getpid());
            fflush(stdout);
        }
    }

    return 0;
}
