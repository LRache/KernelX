#ifndef __KERNELX_FS_EXT4_LINK_H__
#define __KERNELX_FS_EXT4_LINK_H__

#include <fs/ext4/inode.h>

#include <stddef.h>

#ifdef __cplusplus
extern "C" {
#endif

/*
 * Create a hard link named `name` in `parent` that references `child`.
 *
 * Source logic is adapted primarily from lwext4's ext4_link(), restricted to
 * the non-rename hard-link path.
 */
int kernelx_ext4_link(struct ext4_inode_ref *parent,
                      const char *name,
                      size_t name_len,
                      struct ext4_inode_ref *child);

/*
 * Read symlink payload from an inode reference.
 */
int kernelx_ext4_inode_ref_readlink(struct ext4_inode_ref *ref,
                                    void *buf,
                                    size_t size,
                                    size_t *rcnt);

/*
 * Set symlink payload on an inode reference.
 *
 * Source logic is adapted primarily from lwext4's ext4_fsymlink_set(),
 * but reshaped to a direct inode_ref API.
 */
int kernelx_ext4_inode_ref_set_symlink(struct ext4_inode_ref *ref,
                                       const void *buf,
                                       size_t size);

#ifdef __cplusplus
}
#endif

#endif
