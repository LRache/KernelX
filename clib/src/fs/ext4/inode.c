/*
 * Extracted from lwext4:
 *   - clib/lib/lwext4/lwext4/src/ext4_fs.c
 *   - original functions:
 *       ext4_fs_inode_checksum()
 *       ext4_fs_verify_inode_csum()
 *       __ext4_fs_get_inode_ref()
 *       ext4_fs_get_inode_ref()
 *
 * This copy keeps the original logic for reading an inode from the on-disk
 * inode table, but uses a KernelX-specific symbol name to avoid colliding
 * with the lwext4 implementation that is already linked into clib.
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

#include <ext4_config.h>
#include <ext4_types.h>
#include <ext4_misc.h>
#include <ext4_errno.h>
#include <ext4_debug.h>
#include <ext4_trans.h>
#include <ext4_fs.h>
#include <ext4_block_group.h>
#include <ext4_super.h>
#include <ext4_crc32.h>
#include <ext4_inode.h>

#if CONFIG_META_CSUM_ENABLE
static uint32_t kernelx_ext4_inode_checksum(struct ext4_inode_ref *inode_ref)
{
    uint32_t checksum = 0;
    struct ext4_sblock *sb = &inode_ref->fs->sb;
    uint16_t inode_size = ext4_get16(sb, inode_size);

    if (ext4_sb_feature_ro_com(sb, EXT4_FRO_COM_METADATA_CSUM)) {
        uint32_t orig_checksum;
        uint32_t ino_index = to_le32(inode_ref->index);
        uint32_t ino_gen = to_le32(ext4_inode_get_generation(inode_ref->inode));

        /* Zero the stored checksum while recomputing it. */
        orig_checksum = ext4_inode_get_csum(sb, inode_ref->inode);
        ext4_inode_set_csum(sb, inode_ref->inode, 0);

        checksum = ext4_crc32c(EXT4_CRC32_INIT, sb->uuid, sizeof(sb->uuid));
        checksum = ext4_crc32c(checksum, &ino_index, sizeof(ino_index));
        checksum = ext4_crc32c(checksum, &ino_gen, sizeof(ino_gen));
        checksum = ext4_crc32c(checksum, inode_ref->inode, inode_size);
        ext4_inode_set_csum(sb, inode_ref->inode, orig_checksum);

        if (inode_size == EXT4_GOOD_OLD_INODE_SIZE) {
            checksum &= 0xFFFF;
        }
    }

    return checksum;
}

static bool kernelx_ext4_verify_inode_csum(struct ext4_inode_ref *inode_ref)
{
    struct ext4_sblock *sb = &inode_ref->fs->sb;

    if (!ext4_sb_feature_ro_com(sb, EXT4_FRO_COM_METADATA_CSUM)) {
        return true;
    }

    return ext4_inode_get_csum(sb, inode_ref->inode) ==
           kernelx_ext4_inode_checksum(inode_ref);
}
#else
#define kernelx_ext4_verify_inode_csum(...) true
#endif

/*
 * Source: lwext4 ext4_fs.c::__ext4_fs_get_inode_ref() /
 * ext4_fs_get_inode_ref()
 */
int kernelx_ext4_read_inode_ref(struct ext4_fs *fs, uint32_t index,
                                struct ext4_inode_ref *ref)
{
    uint32_t inodes_per_group = ext4_get32(&fs->sb, inodes_per_group);
    struct ext4_block_group_ref bg_ref;
    uint16_t inode_size;
    uint32_t block_group;
    uint32_t offset_in_group;
    uint32_t block_size;
    uint32_t byte_offset_in_group;
    uint32_t offset_in_block;
    ext4_fsblk_t inode_table_start;
    ext4_fsblk_t block_id;
    int rc;

    /*
     * Inode numbers are 1-based in ext4, but the table arithmetic is easier
     * to perform with 0-based indices.
     */
    index -= 1;
    block_group = index / inodes_per_group;
    offset_in_group = index % inodes_per_group;

    rc = ext4_fs_get_block_group_ref(fs, block_group, &bg_ref);
    if (rc != EOK) {
        return rc;
    }

    inode_table_start =
        ext4_bg_get_inode_table_first_block(bg_ref.block_group, &fs->sb);

    rc = ext4_fs_put_block_group_ref(&bg_ref);
    if (rc != EOK) {
        return rc;
    }

    inode_size = ext4_get16(&fs->sb, inode_size);
    block_size = ext4_sb_get_block_size(&fs->sb);
    byte_offset_in_group = offset_in_group * inode_size;
    block_id = inode_table_start + (byte_offset_in_group / block_size);

    rc = ext4_trans_block_get(fs->bdev, &ref->block, block_id);
    if (rc != EOK) {
        return rc;
    }

    offset_in_block = byte_offset_in_group % block_size;
    ref->inode = (struct ext4_inode *)(ref->block.data + offset_in_block);
    ref->index = index + 1;
    ref->fs = fs;
    ref->dirty = false;

    if (!kernelx_ext4_verify_inode_csum(ref)) {
        ext4_dbg(DEBUG_FS,
                 DBG_WARN "Inode checksum failed. Inode: %" PRIu32 "\n",
                 ref->index);
    }

    return EOK;
}
