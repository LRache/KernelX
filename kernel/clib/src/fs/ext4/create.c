#include "ext4.h"
#include "ext4_dir.h"
#include "ext4_errno.h"
#include "ext4_fs.h"
#include "ext4_types.h"

#include <string.h>

int kernelx_ext4_create_inode(
    struct ext4_inode_ref *parent,
    const char *name,
    uint32_t mode
) {
    struct ext4_fs *fs = parent->fs;
    struct ext4_inode_ref child_ref;
    int r;

    r = ext4_fs_alloc_inode(fs, &child_ref, EXT4_DE_REG_FILE);
    if (r != EOK) {
        return -r;
    }

    ext4_fs_inode_blocks_init(fs, &child_ref);
    
    r = ext4_dir_add_entry(parent, name, strlen(name), &child_ref);
    if (r != EOK) {
        ext4_fs_free_inode(&child_ref);
        child_ref.dirty = false;
        ext4_fs_put_inode_ref(&child_ref);
        return -r;
    }

    ext4_fs_inode_links_count_inc(&child_ref);
    child_ref.dirty = true;

    ext4_fs_put_inode_ref(&child_ref);
    
    return EOK;
}
