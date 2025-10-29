#include <stdio.h>
#include <sys/wait.h>
#include <sys/syscall.h>
#include <unistd.h>

void *my_brk(void *addr) {
    return (void *)syscall(SYS_brk, addr);
}

int main() {
    int *top = my_brk(NULL);
    printf("Before brk: %p\n", top);

    int *new_top = my_brk(top + 1024);
    printf("After brk: %p\n", new_top);

    *top = 0x12;
    *(top + 1) = 0x34;

    pid_t pid;
    if ((pid = fork()) == 0) {
        printf("[Children]*%p = %x\n", top, *top);
        *(top + 1) = 0x56;
        printf("[Children]*%p = %x\n", top + 1, *(top + 1));
        return 0;
    } else {
        printf("[Parent]*%p = %x\n", top, *top);
        wait4(pid, NULL, 0, NULL);
        printf("[Parent]*%p = %x\n", top + 1, *(top + 1));
    }

    return 0;
}
