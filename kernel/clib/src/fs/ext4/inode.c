#include "ext4.h"
#include "ext4_blockdev.h"
#include "ext4_errno.h"
#include "ext4_fs.h"
#include "ext4_dir.h"

#include <stdbool.h>
#include <stddef.h>
#include <stdint.h>
#include <stdlib.h>
#include <string.h>


int kernelx_ext4_get_inode(
    struct ext4_fs *fs,
    uint32_t ino,
    struct ext4_inode_ref **ret_inode
) {
    struct ext4_inode_ref *inode_ref = (struct ext4_inode_ref *)malloc(sizeof(struct ext4_inode_ref));
    if (inode_ref == NULL) {
        return -ENOMEM;
    }

    // ext4_fwrite()
    
    int rc = ext4_fs_get_inode_ref(fs, ino, inode_ref);
    if (rc != EOK) {
        free(inode_ref);
        return -rc;
    }

    *ret_inode = inode_ref;

    return rc;
}

int kernelx_ext4_put_inode(
    struct ext4_inode_ref *inode_ref
) {
    int rc = ext4_fs_put_inode_ref(inode_ref);
    if (rc != EOK) {
        return -rc;
    }

    free(inode_ref);
    return EOK;
}

int kernelx_ext4_inode_lookup(
    struct ext4_inode_ref *inode,
    const char *name,
    uint32_t *ret_ino
) {
    struct ext4_dir_search_result result;
    int rc;

    ext4_block_cache_write_back(inode->fs->bdev, 1);

    rc = ext4_dir_find_entry(&result, inode, name, strlen(name));
    if (rc != EOK) {
        ext4_dir_destroy_result(inode, &result);
        return -rc;
    }
    
    *ret_ino = ext4_dir_en_get_inode(result.dentry);

    ext4_dir_destroy_result(inode, &result);

    ext4_block_cache_write_back(inode->fs->bdev, 0);

    return EOK;
}