#include <fcntl.h>
#include <poll.h>
#include <stdio.h>
#include <syscall.h>
#include <unistd.h>

#define __NR_ppoll_time32 73

struct timespec32 {
    int tv_sec;
    int tv_nsec;
};

static int ppoll_time32(struct pollfd *fds, nfds_t nfds, const struct timespec32 *tmo_p) {
    return syscall(__NR_ppoll_time32, fds, nfds, tmo_p, NULL, 0);
}

static int test_random_access_file(void) {
    int fd = open("/dev/null", O_RDWR);
    if (fd < 0) {
        perror("open /dev/null");
        return 1;
    }

    struct pollfd pfd = {
        .fd = fd,
        .events = POLLIN | POLLOUT,
        .revents = 0,
    };

    if (ppoll_time32(&pfd, 1, NULL) != 1) {
        perror("ppoll_time32 /dev/null");
        close(fd);
        return 1;
    }

    if ((pfd.revents & (POLLIN | POLLOUT)) != (POLLIN | POLLOUT)) {
        fprintf(stderr, "unexpected /dev/null revents: %d\n", pfd.revents);
        close(fd);
        return 1;
    }

    close(fd);
    return 0;
}

static int test_pipe_read_hup_combo(void) {
    int pipefd[2];
    if (pipe(pipefd) < 0) {
        perror("pipe");
        return 1;
    }

    if (write(pipefd[1], "x", 1) != 1) {
        perror("write pipe");
        close(pipefd[0]);
        close(pipefd[1]);
        return 1;
    }
    close(pipefd[1]);

    struct pollfd pfd = {
        .fd = pipefd[0],
        .events = POLLIN,
        .revents = 0,
    };

    if (ppoll_time32(&pfd, 1, NULL) != 1) {
        perror("ppoll_time32 pipe");
        close(pipefd[0]);
        return 1;
    }

    if ((pfd.revents & (POLLIN | POLLHUP)) != (POLLIN | POLLHUP)) {
        fprintf(stderr, "unexpected pipe revents: %d\n", pfd.revents);
        close(pipefd[0]);
        return 1;
    }

    close(pipefd[0]);
    return 0;
}

int main(void) {
    if (test_random_access_file() != 0) {
        return 1;
    }
    if (test_pipe_read_hup_combo() != 0) {
        return 1;
    }
    return 0;
}
