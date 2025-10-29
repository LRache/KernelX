#include <stdio.h>
#include <poll.h>
#include <sys/poll.h>
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
    return syscall(__NR_ppoll_time32, fds, nfds, tmo_p, NULL, 0);
}

int main() {
    pid_t pid;

    int pipe0[2], pipe1[2];
    if (pipe(pipe0) < 0 || pipe(pipe1) < 0) {
        perror("pipe");
        return 1;
    }

    pid = fork();
    if (pid < 0) {
        perror("fork");
        return 1;
    }

    if (pid == 0) {
        // Close both write ends in child
        close(pipe0[1]);
        close(pipe1[1]);

        // poll on read ends
        struct pollfd pfds[2];
        pfds[0].fd = pipe0[0];
        pfds[0].events = POLLIN;
        pfds[1].fd = pipe1[0];
        pfds[1].events = POLLIN;

        ppoll_time32(pfds, 2, NULL);

        sleep(2); // Ensure parent is waiting in ppoll_time32

        ppoll_time32(pfds, 1, NULL);

        return 0;
    }

    sleep(1); // Ensure child is waiting in ppoll_time32

    write(pipe0[1], "x", 1); // Wake up child

    sleep(1); // Ensure child is waiting in ppoll_time32 again

    write(pipe1[1], "y", 1); // Wake up child again

    wait(NULL);

    return 0;
}
