#include <fcntl.h>
#include <stdio.h>
#include <sys/syscall.h>
#include <unistd.h>

#ifndef SYS_finit_module
#define SYS_finit_module 273
#endif

#ifndef SYS_delete_module
#define SYS_delete_module 106
#endif

#ifndef MODULE_PATH
#define MODULE_PATH "/tests/kmodule/hello.ko"
#endif

int main(int argc, char **argv) {
    const char *module_path = argc > 1 ? argv[1] : MODULE_PATH;
    int fd = open(module_path, O_RDONLY);
    if (fd < 0) {
        perror("open");
        return 1;
    }

    long ret = syscall(SYS_finit_module, fd, "", 0);
    close(fd);

    if (ret < 0) {
        perror("finit_module");
        return 1;
    }
    if (ret != 0) {
        fprintf(stderr, "finit_module returned %ld\n", ret);
        return 1;
    }

    ret = syscall(SYS_delete_module, "hello", 0);
    if (ret < 0) {
        perror("delete_module");
        return 1;
    }

    puts("hello");
    return 0;
}
