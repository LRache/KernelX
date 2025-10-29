#include <stdio.h>
#include <unistd.h>
#include <poll.h>

int main() {
    int pipefd[2];
    char buffer[20];
    if (pipe(pipefd) == -1) {
        perror("pipe");
        return 1;
    }

    pid_t pid = fork();
    if (pid == -1) {
        perror("fork");
        return 1;
    }

    if (pid == 0) { // Child process
        close(pipefd[0]); // Close unused read end
        const char *msg = "Hello, Pipe!";
        write(pipefd[1], msg, 12);
        close(pipefd[1]); // Close write end after writing
    } else { // Parent process
        close(pipefd[1]); // Close unused write end
        read(pipefd[0], buffer, sizeof(buffer));
        printf("Received message: %s\n", buffer);
        close(pipefd[0]); // Close read end after reading
    }

    return 0;
}
