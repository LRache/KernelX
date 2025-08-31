#include "ext4_inode.h"
#include "ext4_fs.h"

#include <sys/errno.h>
#include <sys/stat.h>
#include <sys/types.h>

ssize_t kernelx_ext4_get_inode_size(struct ext4_inode_ref *inode_ref) {
    struct ext4_fs *const fs = inode_ref->fs;
    struct ext4_sblock *const sb = &fs->sb;

    ssize_t size = ext4_inode_get_size(sb, inode_ref->inode);
    return size;
}
