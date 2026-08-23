#include <fcntl.h>
#include <stdio.h>
#include <string.h>
#include <sys/syscall.h>
#include <unistd.h>

#ifndef SYS_finit_module
#define SYS_finit_module 273
#endif

#ifndef MODULE_PATH
#define MODULE_PATH "/tests/kmodule/devfs_inode.ko"
#endif

#define DEVFS_NODE_PATH "/dev/kmodule_hello"
#define EXPECTED_CONTENT "hello"

int main(int argc, char **argv) {
    const char *module_path = argc > 1 ? argv[1] : MODULE_PATH;
    int module_fd = open(module_path, O_RDONLY);
    if (module_fd < 0) {
        perror("open module");
        return 1;
    }

    long ret = syscall(SYS_finit_module, module_fd, "", 0);
    close(module_fd);
    if (ret < 0) {
        perror("finit_module");
        return 1;
    }
    if (ret != 0) {
        fprintf(stderr, "finit_module returned %ld\n", ret);
        return 1;
    }

    int inode_fd = open(DEVFS_NODE_PATH, O_RDONLY);
    if (inode_fd < 0) {
        perror("open devfs inode");
        return 1;
    }

    char buffer[sizeof(EXPECTED_CONTENT)] = {0};
    ssize_t len = read(inode_fd, buffer, sizeof(buffer));
    if (len != (ssize_t)(sizeof(EXPECTED_CONTENT) - 1) ||
        memcmp(buffer, EXPECTED_CONTENT, sizeof(EXPECTED_CONTENT) - 1) != 0) {
        fprintf(stderr, "unexpected devfs content: len=%zd value=%.*s\n", len, len > 0 ? (int)len : 0, buffer);
        close(inode_fd);
        return 1;
    }

    if (read(inode_fd, buffer, sizeof(buffer)) != 0) {
        fputs("devfs inode did not reach EOF\n", stderr);
        close(inode_fd);
        return 1;
    }
    close(inode_fd);

    /* The devfs inode retains callbacks into this module until unregister is supported. */
    puts("hello");
    return 0;
}
