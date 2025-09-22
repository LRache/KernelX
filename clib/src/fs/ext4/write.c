#include "ext4.h"
#include "ext4_blockdev.h"
#include "ext4_super.h"
#include "ext4_fs.h"
#include "ext4_inode.h"
#include "ext4_errno.h"

#include <stdint.h>
#include <string.h>
#include <sys/types.h>

// reference: ext4_fwrite
ssize_t kernelx_ext4_inode_writeat(
    struct ext4_inode_ref *inode_ref,
    const void *buf,
    size_t size,
    size_t fpos
) {
	// ext4_fwrite(NULL, NULL, 0, NULL);
    uint32_t unalg;
	uint32_t iblk_idx;
	uint32_t iblock_last;
	uint32_t ifile_blocks;
	uint32_t block_size;
    uint64_t file_fsize;

	uint32_t fblock_count;
	ext4_fsblk_t fblk;
	ext4_fsblk_t fblock_start;

    struct ext4_fs *const fs = inode_ref->fs;
	struct ext4_sblock *const sb = &fs->sb;
    const uint8_t *u8_buf = buf;
    int r;
    ssize_t wcnt = 0;

    file_fsize = ext4_inode_get_size(sb, inode_ref->inode);
    block_size = ext4_sb_get_block_size(sb);

    // iblock_last = (uint32_t)((file->fpos + size) / block_size);
	// iblk_idx = (uint32_t)(file->fpos / block_size);
	// ifile_blocks = (uint32_t)((file->fsize + block_size - 1) / block_size);
    iblock_last = (uint32_t)((fpos + size) / block_size);
	iblk_idx = (uint32_t)(fpos / block_size);
	ifile_blocks = (uint32_t)((file_fsize + block_size - 1) / block_size);

    // unalg = (file->fpos) % block_size;
    unalg = fpos % block_size;

    if (unalg) {
        size_t len = size;
        uint64_t off;

        if (size > (block_size - unalg))
            len = block_size - unalg;

        r = ext4_fs_init_inode_dblk_idx(inode_ref, iblk_idx, &fblk);
        if (r != EOK)
            return -r;

        off = fblk * block_size + unalg;
        r = ext4_block_writebytes(fs->bdev, off, u8_buf, len);
        if (r != EOK)
            return -r;

        u8_buf += len;
        size -= len;
        fpos += len;
        wcnt += len;

        iblk_idx++;
    }

    /*Start write back cache mode.*/
	r = ext4_block_cache_write_back(fs->bdev, 1);
	if (r != EOK)
		return -r;

	fblock_start = 0;
	fblock_count = 0;
    while (size >= block_size) {
        int rr;
		while (iblk_idx < iblock_last) {
			if (iblk_idx < ifile_blocks) {
				r = ext4_fs_init_inode_dblk_idx(inode_ref, iblk_idx,
								&fblk);
				if (r != EOK) {
                    return -r;
                }
			} else {
				rr = ext4_fs_append_inode_dblk(inode_ref, &fblk,
							       &iblk_idx);
				if (rr != EOK) {
					/* Unable to append more blocks. But
					 * some block might be allocated already
					 * */
					break;
				}
			}

			iblk_idx++;

			if (!fblock_start) {
				fblock_start = fblk;
			}

			if ((fblock_start + fblock_count) != fblk)
				break;

			fblock_count++;
		}

		// r = ext4_blocks_set_direct(file->mp->fs.bdev, u8_buf, fblock_start,
		// 			   fblock_count);
        r = ext4_blocks_set_direct(fs->bdev, u8_buf, fblock_start,
                       fblock_count);
		if (r != EOK)
			break;

		size -= block_size * fblock_count;
		u8_buf += block_size * fblock_count;
		// file->fpos += block_size * fblock_count;
        fpos += block_size * fblock_count;

		// if (wcnt)
		// 	*wcnt += block_size * fblock_count;
        wcnt += block_size * fblock_count;

		fblock_start = fblk;
		fblock_count = 1;

		if (rr != EOK) {
			/*ext4_fs_append_inode_block has failed and no
			 * more blocks might be written. But node size
			 * should be updated.*/
			r = rr;
			goto out_fsize;
		}
	}

	/*Stop write back cache mode*/
	// ext4_block_cache_write_back(file->mp->fs.bdev, 0);
    ext4_block_cache_write_back(fs->bdev, 0);

	if (r != EOK) {
        return -r;
    }

	if (size) {
		uint64_t off;
		if (iblk_idx < ifile_blocks) {
			// r = ext4_fs_init_inode_dblk_idx(&ref, iblk_idx, &fblk);
			// if (r != EOK)
			// 	goto Finish;
            r = ext4_fs_init_inode_dblk_idx(inode_ref, iblk_idx, &fblk);
            if (r != EOK)
                return -r;
		} else {
			// r = ext4_fs_append_inode_dblk(&ref, &fblk, &iblk_idx);
            r = ext4_fs_append_inode_dblk(inode_ref, &fblk, &iblk_idx);
			if (r != EOK)
				/*Node size sholud be updated.*/
				goto out_fsize;
		}

		off = fblk * block_size;
		// r = ext4_block_writebytes(file->mp->fs.bdev, off, u8_buf, size);
		// if (r != EOK)
		// 	goto Finish;
        r = ext4_block_writebytes(fs->bdev, off, u8_buf, size);
        if (r != EOK)
            return -r;

		// file->fpos += size;
		fpos += size;

		// if (wcnt)
		// 	*wcnt += size;
        wcnt += size;
	}

out_fsize:
	// if (file->fpos > file->fsize) {
	// 	file->fsize = file->fpos;
	// 	ext4_inode_set_size(ref.inode, file->fsize);
	// 	ref.dirty = true;
	// }
    if (fpos > file_fsize) {
        file_fsize = fpos;
        ext4_inode_set_size(inode_ref->inode, file_fsize);
        inode_ref->dirty = true;
    }

    return wcnt;
}