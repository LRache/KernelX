/*
 * Extracted from lwext4:
 *   - clib/lib/lwext4/lwext4/src/ext4.c
 *   - original functions:
 *       ext4_fread()
 *
 * This copy keeps the original logic for reading file data and uses a
 * KernelX-specific symbol name to avoid colliding with the lwext4
 * implementation that is already linked into clib.
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
#include <ext4_blockdev.h>
#include <ext4_types.h>
#include <ext4_errno.h>
#include <ext4_fs.h>
#include <ext4_super.h>
#include <ext4_inode.h>

#include <string.h>

/*
 * Source: lwext4 ext4.c::ext4_fread()
 *
 * Adapted from the file-descriptor based API to a direct inode read-at API.
 */
int kernelx_ext4_inode_ref_read_at(struct ext4_inode_ref *ref,
                                   void *buf,
                                   size_t size,
                                   uint64_t offset,
                                   size_t *rcnt)
{
    struct ext4_fs *fs = ref->fs;
    struct ext4_sblock *sb = &fs->sb;
    uint64_t file_size;
    uint32_t block_size;
    uint8_t *u8_buf = buf;
    size_t done = 0;
    int rc;

    if (rcnt) {
        *rcnt = 0;
    }

    if (size == 0) {
        return EOK;
    }

    if (ext4_inode_is_type(sb, ref->inode, EXT4_INODE_MODE_DIRECTORY)) {
        return EISDIR;
    }

    file_size = ext4_inode_get_size(sb, ref->inode);
    if (offset >= file_size) {
        return EOK;
    }

    if (ext4_inode_is_type(sb, ref->inode, EXT4_INODE_MODE_SOFTLINK) &&
        file_size < sizeof(ref->inode->blocks) &&
        !ext4_inode_get_blocks_count(sb, ref->inode)) {
        size_t len = size;
        uint64_t available = file_size - offset;
        uint8_t *content = (uint8_t *)ref->inode->blocks;

        if ((uint64_t)len > available) {
            len = (size_t)available;
        }

        memcpy(u8_buf, content + offset, len);

        if (rcnt) {
            *rcnt = len;
        }

        return EOK;
    }

    if ((uint64_t)size > file_size - offset) {
        size = (size_t)(file_size - offset);
    }

    block_size = ext4_sb_get_block_size(sb);
    if (((offset + size - 1) / block_size) > UINT32_MAX) {
        return EFBIG;
    }

    while (done < size) {
        uint64_t current_offset = offset + done;
        ext4_lblk_t iblock = (ext4_lblk_t)(current_offset / block_size);
        uint32_t in_block = (uint32_t)(current_offset % block_size);
        ext4_fsblk_t fblock;
        size_t chunk = size - done;

        if (chunk > block_size - in_block) {
            chunk = block_size - in_block;
        }

        rc = ext4_fs_get_inode_dblk_idx(ref, iblock, &fblock, true);
        if (rc != EOK) {
            return rc;
        }

        if (fblock != 0) {
            uint64_t block_offset = fblock * block_size + in_block;

            rc = ext4_block_readbytes(fs->bdev, block_offset, u8_buf + done,
                                      (uint32_t)chunk);
            if (rc != EOK) {
                return rc;
            }
        } else {
            memset(u8_buf + done, 0, chunk);
        }

        done += chunk;
    }

    if (rcnt) {
        *rcnt = done;
    }

    return EOK;
}
