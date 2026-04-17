#ifndef __KERNELX_FS_EXT4_WRITE_H__
#define __KERNELX_FS_EXT4_WRITE_H__

#include <fs/ext4/inode.h>

#include <stddef.h>
#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

/*
 * Write file data to an inode reference at the given byte offset.
 *
 * Source logic is adapted primarily from lwext4's ext4_fwrite(), but reshaped
 * to a direct inode_ref + offset API for KernelX.
 */
int kernelx_ext4_inode_ref_write_at(struct ext4_inode_ref *ref,
                                    const void *buf,
                                    size_t size,
                                    uint64_t offset,
                                    size_t *wcnt);

#ifdef __cplusplus
}
#endif

#endif
