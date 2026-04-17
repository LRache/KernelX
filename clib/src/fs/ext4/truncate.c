/*
 * Extracted from lwext4:
 *   - clib/lib/lwext4/lwext4/src/ext4_fs.c
 *   - original functions:
 *       ext4_fs_release_inode_block()
 *       ext4_fs_truncate_inode()
 *
 * This copy keeps the original inode truncate logic, but adapts it to a
 * KernelX-specific symbol name so it can coexist with the embedded lwext4.
 */

/*
 * Copyright (c) 2013 Grzegorz Kostka (kostka.grzegorz@gmail.com)
 *
 *
 * HelenOS:
 * Copyright (c) 2012 Martin Sucha
 * Copyright (c) 2012 Frantisek Princ
 * All rights reserved.
 *
 * Redistribution and use in source and binary forms, with or without
 * modification, are permitted provided that the following conditions
 * are met:
 *
 * - Redistributions of source code must retain the above copyright
 *   notice, this list of conditions and the following disclaimer.
 * - Redistributions in binary form must reproduce the above copyright
 *   notice, this list of conditions and the following disclaimer in the
 *   documentation and/or other materials provided with the distribution.
 * - The name of the author may not be used to endorse or promote products
 *   derived from this software without specific prior written permission.
 *
 * THIS SOFTWARE IS PROVIDED BY THE AUTHOR ``AS IS'' AND ANY EXPRESS OR
 * IMPLIED WARRANTIES, INCLUDING, BUT NOT LIMITED TO, THE IMPLIED WARRANTIES
 * OF MERCHANTABILITY AND FITNESS FOR A PARTICULAR PURPOSE ARE DISCLAIMED.
 * IN NO EVENT SHALL THE AUTHOR BE LIABLE FOR ANY DIRECT, INDIRECT,
 * INCIDENTAL, SPECIAL, EXEMPLARY, OR CONSEQUENTIAL DAMAGES (INCLUDING, BUT
 * NOT LIMITED TO, PROCUREMENT OF SUBSTITUTE GOODS OR SERVICES; LOSS OF USE,
 * DATA, OR PROFITS; OR BUSINESS INTERRUPTION) HOWEVER CAUSED AND ON ANY
 * THEORY OF LIABILITY, WHETHER IN CONTRACT, STRICT LIABILITY, OR TORT
 * (INCLUDING NEGLIGENCE OR OTHERWISE) ARISING IN ANY WAY OUT OF THE USE OF
 * THIS SOFTWARE, EVEN IF ADVISED OF THE POSSIBILITY OF SUCH DAMAGE.
 */

#include <fs/ext4.h>

#include <ext4_balloc.h>
#include <ext4_debug.h>
#include <ext4_errno.h>
#include <ext4_extent.h>
#include <ext4_fs.h>
#include <ext4_inode.h>
#include <ext4_super.h>
#include <ext4_trans.h>

#include <string.h>

static int kernelx_ext4_release_inode_block(struct ext4_inode_ref *inode_ref,
                                            ext4_lblk_t iblock)
{
    ext4_fsblk_t fblock;
    struct ext4_fs *fs = inode_ref->fs;

    ext4_assert(!(
        ext4_sb_feature_incom(&fs->sb, EXT4_FINCOM_EXTENTS) &&
        (ext4_inode_has_flag(inode_ref->inode, EXT4_INODE_FLAG_EXTENTS))));

    struct ext4_inode *inode = inode_ref->inode;

    if (iblock < EXT4_INODE_DIRECT_BLOCK_COUNT) {
        fblock = ext4_inode_get_direct_block(inode, iblock);
        if (fblock == 0) {
            return EOK;
        }

        ext4_inode_set_direct_block(inode, iblock, 0);
        return ext4_balloc_free_block(inode_ref, fblock);
    }

    {
        unsigned int level = 0;
        unsigned int i;

        for (i = 1; i < 4; i++) {
            if (iblock < fs->inode_block_limits[i]) {
                level = i;
                break;
            }
        }

        if (level == 0) {
            return EIO;
        }

        uint32_t block_offset_in_level =
            (uint32_t)(iblock - fs->inode_block_limits[level - 1]);
        ext4_fsblk_t current_block =
            ext4_inode_get_indirect_block(inode, level - 1);
        uint32_t offset_in_block =
            (uint32_t)(block_offset_in_level /
                       fs->inode_blocks_per_level[level - 1]);
        struct ext4_block block;

        while (level > 0) {
            int rc;

            if (current_block == 0) {
                return EOK;
            }

            rc = ext4_trans_block_get(fs->bdev, &block, current_block);
            if (rc != EOK) {
                return rc;
            }

            current_block =
                to_le32(((uint32_t *)block.data)[offset_in_block]);

            if (level == 1) {
                ((uint32_t *)block.data)[offset_in_block] = to_le32(0);
                ext4_trans_set_block_dirty(block.buf);
            }

            rc = ext4_block_set(fs->bdev, &block);
            if (rc != EOK) {
                return rc;
            }

            level--;
            if (level == 0) {
                break;
            }

            block_offset_in_level %= fs->inode_blocks_per_level[level];
            offset_in_block =
                (uint32_t)(block_offset_in_level /
                           fs->inode_blocks_per_level[level - 1]);
        }

        fblock = current_block;
    }

    if (fblock == 0) {
        return EOK;
    }

    return ext4_balloc_free_block(inode_ref, fblock);
}

/*
 * Source: lwext4 ext4_fs.c::ext4_fs_truncate_inode()
 */
int kernelx_ext4_inode_ref_truncate(struct ext4_inode_ref *inode_ref,
                                    uint64_t new_size)
{
    struct ext4_sblock *sb = &inode_ref->fs->sb;
    uint32_t i;
    int rc;
    bool is_small_symlink;
    uint64_t old_size;

    if (!ext4_inode_can_truncate(sb, inode_ref->inode)) {
        return EINVAL;
    }

    old_size = ext4_inode_get_size(sb, inode_ref->inode);
    if (old_size == new_size) {
        return EOK;
    }

    if (old_size < new_size) {
        return EINVAL;
    }

    is_small_symlink =
        ext4_inode_is_type(sb, inode_ref->inode, EXT4_INODE_MODE_SOFTLINK);
    if (is_small_symlink &&
        old_size < sizeof(inode_ref->inode->blocks) &&
        !ext4_inode_get_blocks_count(sb, inode_ref->inode)) {
        char *content = (char *)inode_ref->inode->blocks + new_size;

        memset(content, 0,
               sizeof(inode_ref->inode->blocks) - (uint32_t)new_size);
        ext4_inode_set_size(inode_ref->inode, new_size);
        inode_ref->dirty = true;
        return EOK;
    }

    i = ext4_inode_type(sb, inode_ref->inode);
    if (i == EXT4_INODE_MODE_CHARDEV ||
        i == EXT4_INODE_MODE_BLOCKDEV ||
        i == EXT4_INODE_MODE_SOCKET) {
        inode_ref->inode->blocks[0] = 0;
        inode_ref->inode->blocks[1] = 0;
        inode_ref->dirty = true;
        return EOK;
    }

    {
        uint32_t block_size = ext4_sb_get_block_size(sb);
        uint32_t new_blocks_cnt =
            (uint32_t)((new_size + block_size - 1) / block_size);
        uint32_t old_blocks_cnt =
            (uint32_t)((old_size + block_size - 1) / block_size);
        uint32_t diff_blocks_cnt = old_blocks_cnt - new_blocks_cnt;

#if CONFIG_EXTENT_ENABLE && CONFIG_EXTENTS_ENABLE
        if (ext4_sb_feature_incom(sb, EXT4_FINCOM_EXTENTS) &&
            ext4_inode_has_flag(inode_ref->inode, EXT4_INODE_FLAG_EXTENTS)) {
            if (diff_blocks_cnt) {
                rc = ext4_extent_remove_space(inode_ref, new_blocks_cnt,
                                              EXT_MAX_BLOCKS);
                if (rc != EOK) {
                    return rc;
                }
            }
        } else
#endif
        {
            for (i = 0; i < diff_blocks_cnt; ++i) {
                rc = kernelx_ext4_release_inode_block(inode_ref,
                                                      new_blocks_cnt + i);
                if (rc != EOK) {
                    return rc;
                }
            }
        }
    }

    ext4_inode_set_size(inode_ref->inode, new_size);
    inode_ref->dirty = true;
    return EOK;
}
