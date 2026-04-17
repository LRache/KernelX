#ifndef __KERNELX_FS_EXT4_INODE_H__
#define __KERNELX_FS_EXT4_INODE_H__

#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

struct ext4_fs;
struct ext4_inode_ref;

/*
 * Extracted implementation lives in clib/src/fs/ext4/inode.c and is derived
 * from lwext4's inode lookup path in ext4_fs.c.
 */
int kernelx_ext4_read_inode_ref(struct ext4_fs *fs, uint32_t index,
                                struct ext4_inode_ref *ref);

#ifdef __cplusplus
}
#endif

#endif
