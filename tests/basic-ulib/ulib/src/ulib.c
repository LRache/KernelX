#include "ulib.h"

#include <fcntl.h>
#include <stdint.h>
#include <threads.h>
#include <unistd.h>
#include <sched.h>
#include <sys/syscall.h>
#include <sys/fcntl.h>

void __call_main(int argc, char **argv, char **envp) {
    extern int main(int argc, char **argv, char **envp);

    int result = main(argc, argv, envp);
    _exit(result);
}

void _exit(int code) {
    __syscall(SYS_exit, code, 0, 0, 0);
    while (1);
}

int openat(int dirfd, const char *path, int flags, ...) {
    return __syscall(SYS_openat, dirfd, (uintptr_t)path, flags, 0);
}

int open(const char *__file, int __oflag, ...) {
    return openat(AT_FDCWD, __file, __oflag, 0);
}

ssize_t read(int fd, void *buf, size_t n) {
    return __syscall(SYS_read, fd, (uintptr_t)buf, n, 0);
}

ssize_t write(int fd, const void *buf, size_t n) {
    return __syscall(SYS_write, fd, (uintptr_t)buf, n, 0);
}

int close(int fd) {
    return __syscall(SYS_close, fd, 0, 0, 0);
}

pid_t fork(void) {
    return __syscall(SYS_clone, 0, 0, 0, 0);
}

int execve(const char *path, char *const *argv, char *const *envp) {
    return __syscall(SYS_execve, (uintptr_t)path, (uintptr_t)argv, (uintptr_t)envp, 0);
}

pid_t wait4(pid_t pid, int *status, int options, struct rusage *rusage) {
    return __syscall(SYS_wait4, pid, (uintptr_t)status, options, (uintptr_t)rusage);
}

int sched_yield() {
    return __syscall(SYS_sched_yield, 0, 0, 0, 0);
}

int brk(void *addr) {
    uintptr_t requested = (uintptr_t)addr;
    uintptr_t result = __syscall(SYS_brk, requested, 0, 0, 0);
    
    if (result != requested) {
        return -1;
    }
    return 0;
}

void *sbrk(intptr_t increment) {
    static void *current_brk = NULL;
    
    if (current_brk == NULL) {
        uintptr_t current = __syscall(SYS_brk, 0, 0, 0, 0);
        current_brk = (void *)current;
    }
    
    if (increment == 0) {
        return current_brk;
    }
    
    void *old_brk = current_brk;
    uintptr_t new_addr = (uintptr_t)current_brk + increment;
    
    if (brk((void *)new_addr) < 0) {
        return (void *)-1;
    }
    
    current_brk = (void *)new_addr;
    return old_brk;
}

