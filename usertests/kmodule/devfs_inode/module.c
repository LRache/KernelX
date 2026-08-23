#include "kmodule.h"

#include <errno.h>
#include <sys/stat.h>

#define DEVFS_NODE_NAME "kmodule_hello"
#define HELLO_CONTENT "hello"
#define HELLO_CONTENT_LEN (sizeof(HELLO_CONTENT) - 1)

static const char *hello_type_name(void *data) {
    (void)data;
    return DEVFS_NODE_NAME;
}

static long hello_size(void *data) {
    (void)data;
    return HELLO_CONTENT_LEN;
}

static long hello_mode(void *data) {
    (void)data;
    return S_IFREG | 0444;
}

static long hello_owner(void *data, uint32_t *uid, uint32_t *gid) {
    (void)data;
    *uid = 0;
    *gid = 0;
    return 0;
}

static long hello_readat(void *data, uint8_t *buf, size_t len, size_t offset, bool direct) {
    (void)data;
    (void)direct;

    if (offset >= HELLO_CONTENT_LEN) {
        return 0;
    }

    size_t remaining = HELLO_CONTENT_LEN - offset;
    if (len > remaining) {
        len = remaining;
    }
    for (size_t i = 0; i < len; ++i) {
        buf[i] = HELLO_CONTENT[offset + i];
    }
    return len;
}

static long hello_writeat(void *data, const uint8_t *buf, size_t len, size_t offset) {
    (void)data;
    (void)buf;
    (void)len;
    (void)offset;
    return -EOPNOTSUPP;
}

static BridgeInodeOps hello_inode = {
    .type_name = hello_type_name,
    .size = hello_size,
    .mode = hello_mode,
    .owner = hello_owner,
    .readat = hello_readat,
    .writeat = hello_writeat,
};

static int devfs_inode_init(void) {
    long ino = devfs_register(&hello_inode);
    if (ino < 0) {
        return ino;
    }

    kinfo("registered /dev/%s as inode %ld", DEVFS_NODE_NAME, ino);
    return 0;
}

static void devfs_inode_exit(void) {
    kinfo("devfs inode module exit");
}

KERNELX_MODULE("devfs_inode", devfs_inode_init, devfs_inode_exit);
