#include <stdio.h>
#include <poll.h>
#include <time.h>
#include <syscall.h>
#include <unistd.h>
#include <sys/wait.h>

#define __NR_ppoll_time32 73

struct timespec32 {
    int tv_sec;   // seconds
    int tv_nsec;  // nanoseconds
};

int ppoll_time32(struct pollfd *fds, nfds_t nfds, const struct timespec32 *tmo_p) {
    return syscall(__NR_ppoll, fds, nfds, tmo_p, NULL, 0);
}

void sleep_seconds(int seconds) {
    struct timespec req = {seconds, 0};
    nanosleep(&req, NULL);
}

int main() {
    int pipefd[2];
    if (pipe(pipefd) == -1) {
        perror("pipe");
        return 1;
    }

    pid_t pid = fork();
    if (pid == -1) {
        perror("fork");
        return 1;
    }

    if (pid == 0) {
        // Read pipe
        close(pipefd[1]); // Close write end

        sleep_seconds(1);
        printf("after sleep\n");
        fflush(stdout);

        dup2(pipefd[0], 100); // Redirect stdin to read end of pipe

        struct pollfd pfd;
        pfd.fd = 100;
        pfd.events = POLLIN;

        // Should return immediately
        if (ppoll_time32(&pfd, 1, NULL) == -1) {
            perror("ppoll_time32");
        }

        if (pfd.fd != 100 || !(pfd.revents & POLLIN)) {
            fprintf(stderr, "Unexpected poll result: fd=%d, revents=%d\n", pfd.fd, pfd.revents);
            return 1;
        }
        
        char buffer[100];
        ssize_t n = read(pipefd[0], buffer, sizeof(buffer));
        if (n == -1) {
            perror("read");
            return 1;
        } else {
            buffer[n] = '\0';
            printf("Child read: %s\n", buffer);
        }

        close(pipefd[0]);
    } else {
        // Write pipe
        close(pipefd[0]); // Close read end

        const char *msg = "Hello from parent!";
        if (write(pipefd[1], msg, 18) == -1) {
            perror("write");
            return 1;
        }

        waitpid(pid, NULL, 0);

        close(pipefd[1]);
    }

    return 0;
}
