#include <errno.h>
#include <stdio.h>
#include <string.h>
#include <sys/syscall.h>
#include <sys/wait.h>
#include <time.h>
#include <unistd.h>

#define __NR_pselect6_time32 72
#define FD_SET_SIZE 1024

struct timespec32 {
	int tv_sec;
	int tv_nsec;
};

struct kernel_fd_set {
	unsigned long fds_bits[FD_SET_SIZE / (8 * sizeof(unsigned long))];
};

static inline void k_fd_zero(struct kernel_fd_set *set) {
	memset(set, 0, sizeof(*set));
}

static inline void k_fd_set(int fd, struct kernel_fd_set *set) {
	size_t bits_per_long = sizeof(unsigned long) * 8;
	size_t index = fd / bits_per_long;
	size_t bit = fd % bits_per_long;
	set->fds_bits[index] |= (1UL << bit);
}

static inline int k_fd_isset(int fd, const struct kernel_fd_set *set) {
	size_t bits_per_long = sizeof(unsigned long) * 8;
	size_t index = fd / bits_per_long;
	size_t bit = fd % bits_per_long;
	return (set->fds_bits[index] & (1UL << bit)) != 0;
}

static int pselect_time32(int nfds,
						  struct kernel_fd_set *readfds,
						  struct kernel_fd_set *writefds,
						  struct kernel_fd_set *exceptfds,
						  const struct timespec32 *timeout) {
	return syscall(__NR_pselect6_time32, nfds, readfds, writefds, exceptfds, timeout, NULL);
}

int main(void) {
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
		close(pipefd[1]);

		struct kernel_fd_set readfds;
		k_fd_zero(&readfds);
		k_fd_set(pipefd[0], &readfds);

		struct timespec32 timeout = { .tv_sec = 5, .tv_nsec = 0 };

		int ready = pselect_time32(pipefd[0] + 1, &readfds, NULL, NULL, &timeout);
		if (ready == -1) {
			perror("pselect_time32");
			return 1;
		}

		if (ready != 1 || !k_fd_isset(pipefd[0], &readfds)) {
			fprintf(stderr, "Unexpected pselect result: ready=%d, isset=%d\n",
					ready, k_fd_isset(pipefd[0], &readfds));
			return 1;
		}

		char buffer[32];
		ssize_t n = read(pipefd[0], buffer, sizeof(buffer) - 1);
		if (n < 0) {
			perror("read");
			return 1;
		}
		buffer[n] = '\0';
		printf("child read: %s\n", buffer);
		fflush(stdout);

		k_fd_zero(&readfds);
		k_fd_set(pipefd[0], &readfds);
		struct timespec32 short_timeout = { .tv_sec = 0, .tv_nsec = 200000000 };
		ready = pselect_time32(pipefd[0] + 1, &readfds, NULL, NULL, &short_timeout);
		if (ready == -1) {
			perror("pselect_time32");
			return 1;
		}

		if (ready != 0) {
			fprintf(stderr, "Expected timeout, got %d\n", ready);
			return 1;
		}

		close(pipefd[0]);
		return 0;
	} else {
		close(pipefd[0]);

		sleep(1); // allow child to block in pselect
		const char *msg = "hello from parent";
		if (write(pipefd[1], msg, strlen(msg)) == -1) {
			perror("write");
			return 1;
		}

		close(pipefd[1]);

		int status;
		if (waitpid(pid, &status, 0) == -1) {
			perror("waitpid");
			return 1;
		}

		if (!WIFEXITED(status) || WEXITSTATUS(status) != 0) {
			fprintf(stderr, "child exited abnormally: status=%d\n", status);
			return 1;
		}

		return 0;
	}
}
