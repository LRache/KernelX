#ifndef __KERNELX_FS_EXT4_TRUNCATE_H__
#define __KERNELX_FS_EXT4_TRUNCATE_H__

#include <fs/ext4/inode.h>

#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

/*
 * Truncate an inode reference down to the given size.
 *
 * Source logic is adapted primarily from lwext4's
 * ext4_fs_truncate_inode().
 */
int kernelx_ext4_inode_ref_truncate(struct ext4_inode_ref *ref,
                                    uint64_t new_size);

#ifdef __cplusplus
}
#endif

#endif
