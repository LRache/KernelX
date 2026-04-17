#ifndef __KERNELX_FS_EXT4_READ_H__
#define __KERNELX_FS_EXT4_READ_H__

#include <fs/ext4/inode.h>

#include <stddef.h>
#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

/*
 * Read file data from an inode reference at the given byte offset.
 *
 * Source logic is adapted from lwext4's ext4_fread(), but reshaped to a
 * direct inode_ref + offset API.
 */
int kernelx_ext4_inode_ref_read_at(struct ext4_inode_ref *ref,
                                   void *buf,
                                   size_t size,
                                   uint64_t offset,
                                   size_t *rcnt);

#ifdef __cplusplus
}
#endif

#endif
