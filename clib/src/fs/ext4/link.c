/*
 * Extracted from lwext4:
 *   - clib/lib/lwext4/lwext4/src/ext4.c
 *   - original functions:
 *       ext4_link()
 *       ext4_fsymlink_set()
 *       ext4_readlink()
 *
 * This copy keeps the original hard-link and symlink payload logic, but
 * adapts it to direct inode_ref APIs for KernelX.
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
#include <ext4_dir.h>
#include <ext4_blockdev.h>
#include <ext4_errno.h>
#include <ext4_fs.h>
#include <ext4_inode.h>
#include <ext4_super.h>

#include <string.h>

/*
 * Source: lwext4 ext4.c::ext4_link()
 *
 * Adapted to the non-rename hard-link path on already-loaded inode refs.
 */
int kernelx_ext4_link(struct ext4_inode_ref *parent,
                      const char *name,
                      size_t name_len,
                      struct ext4_inode_ref *child)
{
    int rc;

    if (!parent || !child || !name) {
        return EINVAL;
    }

    if (parent->fs != child->fs) {
        return EINVAL;
    }

    if (parent->fs->read_only) {
        return EROFS;
    }

    if (!ext4_inode_is_type(&parent->fs->sb, parent->inode,
                            EXT4_INODE_MODE_DIRECTORY)) {
        return ENOTDIR;
    }

    if (ext4_inode_is_type(&parent->fs->sb, child->inode,
                           EXT4_INODE_MODE_DIRECTORY)) {
        return EINVAL;
    }

    if (name_len > EXT4_DIRECTORY_FILENAME_LEN) {
        return EINVAL;
    }

    rc = ext4_dir_add_entry(parent, name, (uint32_t)name_len, child);
    if (rc != EOK) {
        return rc;
    }

    ext4_fs_inode_links_count_inc(child);
    child->dirty = true;
    return EOK;
}

int kernelx_ext4_inode_ref_readlink(struct ext4_inode_ref *ref,
                                    void *buf,
                                    size_t size,
                                    size_t *rcnt)
{
    if (!ext4_inode_is_type(&ref->fs->sb, ref->inode,
                            EXT4_INODE_MODE_SOFTLINK)) {
        return EINVAL;
    }

    return kernelx_ext4_inode_ref_read_at(ref, buf, size, 0, rcnt);
}

/*
 * Source: lwext4 ext4.c::ext4_fsymlink_set()
 */
int kernelx_ext4_inode_ref_set_symlink(struct ext4_inode_ref *ref,
                                       const void *buf,
                                       size_t size)
{
    struct ext4_fs *fs = ref->fs;
    uint32_t block_size;
    int rc;
    int end_rc;

    if (!ext4_inode_is_type(&fs->sb, ref->inode, EXT4_INODE_MODE_SOFTLINK)) {
        return EINVAL;
    }

    if (fs->read_only) {
        return EROFS;
    }

    block_size = ext4_sb_get_block_size(&fs->sb);
    if (size > block_size) {
        return EINVAL;
    }

    rc = kernelx_ext4_inode_ref_truncate(ref, 0);
    if (rc != EOK) {
        return rc;
    }

    if (size == 0) {
        return EOK;
    }

    rc = ext4_block_cache_write_back(fs->bdev, 1);
    if (rc != EOK) {
        return rc;
    }

    if (size < sizeof(ref->inode->blocks)) {
        memset(ref->inode->blocks, 0, sizeof(ref->inode->blocks));
        memcpy(ref->inode->blocks, buf, size);
        ext4_inode_clear_flag(ref->inode, EXT4_INODE_FLAG_EXTENTS);
    } else {
        ext4_fsblk_t fblock;
        ext4_lblk_t sblock;
        uint64_t off;

        ext4_fs_inode_blocks_init(fs, ref);
        rc = ext4_fs_append_inode_dblk(ref, &fblock, &sblock);
        if (rc == EOK) {
            off = fblock * block_size;
            rc = ext4_block_writebytes(fs->bdev, off, buf, (uint32_t)size);
        }
    }

    end_rc = ext4_block_cache_write_back(fs->bdev, 0);
    if (rc == EOK) {
        rc = end_rc;
    }
    if (rc != EOK) {
        return rc;
    }

    ext4_inode_set_size(ref->inode, size);
    ref->dirty = true;
    return EOK;
}
