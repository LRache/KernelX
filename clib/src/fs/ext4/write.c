/*
 * Extracted from lwext4:
 *   - clib/lib/lwext4/lwext4/src/ext4.c
 *   - original functions:
 *       ext4_fwrite()
 *
 * This copy keeps the original write-path logic, but adapts it to a direct
 * inode_ref + offset API. The hole-extension helper is KernelX-local glue
 * added to support write_at() semantics without mountpoint/file wrappers.
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

#include <ext4_blockdev.h>
#include <ext4_errno.h>
#include <ext4_fs.h>
#include <ext4_inode.h>
#include <ext4_super.h>
#include <ext4_trans.h>

#include <string.h>

struct kernelx_ext4_writeback_guard {
    struct ext4_blockdev *bdev;
};

static int kernelx_ext4_writeback_guard_begin(
    struct kernelx_ext4_writeback_guard *guard,
    struct ext4_blockdev *bdev)
{
    guard->bdev = bdev;
    return ext4_block_cache_write_back(bdev, 1);
}

static int kernelx_ext4_writeback_guard_end(
    struct kernelx_ext4_writeback_guard *guard)
{
    return ext4_block_cache_write_back(guard->bdev, 0);
}

static int kernelx_ext4_zero_fs_block(struct ext4_fs *fs, ext4_fsblk_t fblock)
{
    uint32_t block_size = ext4_sb_get_block_size(&fs->sb);
    uint8_t zeros[4096] = {0};
    uint64_t write_off = (uint64_t)fblock * block_size;
    uint32_t left = block_size;

    while (left > 0) {
        uint32_t len = left > sizeof(zeros) ? sizeof(zeros) : left;
        int rc = ext4_block_writebytes(fs->bdev, write_off, zeros, len);
        if (rc != EOK) {
            return rc;
        }

        write_off += len;
        left -= len;
    }

    return EOK;
}

static int kernelx_ext4_inode_ref_extend_zeros(struct ext4_inode_ref *ref,
                                               uint64_t new_size)
{
    uint64_t old_size = ext4_inode_get_size(&ref->fs->sb, ref->inode);
    uint32_t block_size = ext4_sb_get_block_size(&ref->fs->sb);
    uint32_t old_blocks;
    uint32_t new_blocks;
    uint32_t block;
    int rc;

    if (new_size <= old_size) {
        return EOK;
    }

    old_blocks = (uint32_t)((old_size + block_size - 1) / block_size);
    new_blocks = (uint32_t)((new_size + block_size - 1) / block_size);

    for (block = old_blocks; block < new_blocks; ++block) {
        ext4_fsblk_t fblock = 0;
        ext4_lblk_t new_block = 0;

        rc = ext4_fs_append_inode_dblk(ref, &fblock, &new_block);
        if (rc != EOK) {
            return rc;
        }

        if (new_block != block) {
            return EIO;
        }

        rc = kernelx_ext4_zero_fs_block(ref->fs, fblock);
        if (rc != EOK) {
            return rc;
        }
    }

    if ((old_size % block_size) != 0 || old_size == 0) {
        ext4_lblk_t old_last_block = (ext4_lblk_t)(old_size / block_size);
        uint32_t start = (uint32_t)(old_size % block_size);
        ext4_fsblk_t fblock = 0;
        uint8_t zeros[4096] = {0};
        uint32_t left;
        uint64_t write_off;

        rc = ext4_fs_init_inode_dblk_idx(ref, old_last_block, &fblock);
        if (rc != EOK) {
            return rc;
        }

        if (start < block_size) {
            write_off = fblock * block_size + start;
            left = block_size - start;

            while (left > 0) {
                uint32_t len = left > sizeof(zeros) ? sizeof(zeros) : left;

                rc = ext4_block_writebytes(ref->fs->bdev,
                                           write_off,
                                           zeros,
                                           len);
                if (rc != EOK) {
                    return rc;
                }

                write_off += len;
                left -= len;
            }
        }
    }

    ext4_inode_set_size(ref->inode, new_size);
    ref->dirty = true;
    return EOK;
}

static int kernelx_ext4_inode_ref_get_writable_fblock(
    struct ext4_inode_ref *ref,
    uint32_t block,
    uint32_t existing_blocks,
    ext4_fsblk_t *fblock)
{
    if (block < existing_blocks) {
        return ext4_fs_init_inode_dblk_idx(ref, block, fblock);
    }

    {
        ext4_lblk_t new_block = 0;
        int rc;
        rc = ext4_fs_append_inode_dblk(ref, fblock, &new_block);
        if (rc != EOK) {
            return rc;
        }

        if (new_block != block) {
            return EIO;
        }

        return kernelx_ext4_zero_fs_block(ref->fs, *fblock);
    }
}

int kernelx_ext4_inode_ref_write_at(struct ext4_inode_ref *ref,
                                    const void *buf,
                                    size_t size,
                                    uint64_t offset,
                                    size_t *wcnt)
{
    struct kernelx_ext4_writeback_guard guard;
    struct ext4_fs *fs = ref->fs;
    const uint8_t *u8_buf = buf;
    uint64_t start_offset = offset;
    uint64_t original_size;
    uint64_t file_size;
    uint64_t final_size;
    uint64_t current_size;
    uint32_t block_size;
    uint32_t existing_blocks;
    uint32_t block_start;
    uint64_t end;
    size_t written = 0;
    int rr = EOK;
    int writeback_enabled = 0;
    int rc;
    int end_rc;

    if (wcnt) {
        *wcnt = 0;
    }

    if (fs->read_only) {
        return EROFS;
    }

    if (size == 0) {
        return EOK;
    }

    if (offset > UINT64_MAX - size) {
        return EINVAL;
    }

    if (ext4_inode_is_type(&fs->sb, ref->inode, EXT4_INODE_MODE_DIRECTORY)) {
        rc = EISDIR;
        return rc;
    }

    if (!ext4_inode_is_type(&fs->sb, ref->inode, EXT4_INODE_MODE_FILE)) {
        rc = EINVAL;
        return rc;
    }

    if (!ext4_inode_has_flag(ref->inode, EXT4_INODE_FLAG_EXTENTS)) {
        rc = ENOTSUP;
        return rc;
    }

    block_size = ext4_sb_get_block_size(&fs->sb);
    end = offset + size;
    if (((end - 1) / block_size) > UINT32_MAX) {
        return EFBIG;
    }

    original_size = ext4_inode_get_size(&fs->sb, ref->inode);
    file_size = original_size;
    if (offset > file_size) {
        rc = kernelx_ext4_inode_ref_extend_zeros(ref, offset);
        if (rc != EOK) {
            goto out;
        }
        file_size = offset;
    }

    {
        uint64_t existing_blocks64 = file_size / block_size;
        if ((file_size % block_size) != 0) {
            existing_blocks64++;
        }
        if (existing_blocks64 > UINT32_MAX) {
            return EFBIG;
        }

        existing_blocks = (uint32_t)existing_blocks64;
    }

    block_start = (uint32_t)(offset / block_size);

    if ((offset % block_size) != 0) {
        ext4_fsblk_t fblock = 0;
        size_t len = block_size - (size_t)(offset % block_size);
        if (len > size) {
            len = size;
        }

        rc = kernelx_ext4_inode_ref_get_writable_fblock(ref,
                                                        block_start,
                                                        existing_blocks,
                                                        &fblock);
        if (rc != EOK) {
            goto out;
        }

        rc = ext4_block_writebytes(fs->bdev,
                                   fblock * block_size + (offset % block_size),
                                   u8_buf,
                                   (uint32_t)len);
        if (rc != EOK) {
            goto out;
        }

        u8_buf += len;
        size -= len;
        offset += len;
        written += len;
        if (wcnt) {
            *wcnt += len;
        }
        block_start++;
    }

    rc = kernelx_ext4_writeback_guard_begin(&guard, fs->bdev);
    if (rc != EOK) {
        goto out;
    }
    writeback_enabled = 1;

    while (size >= block_size) {
        ext4_fsblk_t fblock_start = 0;
        uint32_t fblock_count = 0;
        uint32_t block = block_start;
        uint64_t full_blocks = (uint64_t)(size / block_size);

        rr = EOK;
        while (full_blocks > 0) {
            ext4_fsblk_t fblock = 0;

            if (block < existing_blocks) {
                rc = ext4_fs_init_inode_dblk_idx(ref, block, &fblock);
                if (rc != EOK) {
                    goto out_writeback;
                }
            } else {
                ext4_lblk_t new_block = 0;

                rr = ext4_fs_append_inode_dblk(ref, &fblock, &new_block);
                if (rr != EOK) {
                    break;
                }
                if (new_block != block) {
                    rr = EIO;
                    break;
                }
            }

            if (fblock_count == 0) {
                fblock_start = fblock;
            } else if (fblock != fblock_start + fblock_count) {
                break;
            }

            fblock_count++;
            block++;
            full_blocks--;
        }

        if (fblock_count == 0) {
            rc = rr;
            goto out_writeback;
        }

        rc = ext4_blocks_set_direct(fs->bdev, u8_buf, fblock_start,
                                    fblock_count);
        if (rc != EOK) {
            goto out_writeback;
        }

        size -= (size_t)block_size * fblock_count;
        u8_buf += (size_t)block_size * fblock_count;
        offset += (uint64_t)block_size * fblock_count;
        written += (size_t)block_size * fblock_count;
        if (wcnt) {
            *wcnt += (size_t)block_size * fblock_count;
        }

        block_start = block;
        if (block_start > existing_blocks) {
            existing_blocks = block_start;
        }

        if (rr != EOK) {
            rc = rr;
            goto out_writeback;
        }
    }

out_writeback:
    if (writeback_enabled) {
        end_rc = kernelx_ext4_writeback_guard_end(&guard);
        writeback_enabled = 0;
        if (rc == EOK) {
            rc = end_rc;
        }
    }
    if (rc != EOK) {
        goto out;
    }

    if (size != 0) {
        ext4_fsblk_t fblock = 0;

        rc = kernelx_ext4_inode_ref_get_writable_fblock(ref,
                                                        block_start,
                                                        existing_blocks,
                                                        &fblock);
        if (rc != EOK) {
            goto out;
        }

        rc = ext4_block_writebytes(fs->bdev, fblock * block_size, u8_buf,
                                   (uint32_t)size);
        if (rc != EOK) {
            goto out;
        }

        offset += size;
        written += size;
        if (wcnt) {
            *wcnt += size;
        }
    }

    rc = EOK;

out:
    final_size = original_size;
    if (written > 0) {
        uint64_t visible_end = start_offset + written;
        if (visible_end > final_size) {
            final_size = visible_end;
        }
    }

    current_size = ext4_inode_get_size(&fs->sb, ref->inode);
    if (current_size > final_size) {
        end_rc = kernelx_ext4_inode_ref_truncate(ref, final_size);
        if (rc == EOK) {
            rc = end_rc;
        }
    } else if (current_size < final_size) {
        ext4_inode_set_size(ref->inode, final_size);
        ref->dirty = true;
    }

    if (rc != EOK && written > 0) {
        return EOK;
    }

    return rc;
}
