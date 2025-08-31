# include <stdio.h>
# include <unistd.h>

int main() {
    if (fork() == 0) {
        // This is the child process
        puts("child: Hello from child process!");
    } else {
        // This is the parent process
        puts("child: Hello from parent process!");
    }
    // puts("Hello from child process!");
    return 0;
}