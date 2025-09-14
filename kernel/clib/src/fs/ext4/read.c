#include "ext4.h"
#include "ext4_super.h"
#include "ext4_fs.h"
#include "ext4_inode.h"
#include "ext4_errno.h"

#include <string.h>
#include <sys/types.h>

static size_t min(size_t a, size_t b) {
    return (a < b) ? a : b;
}

ssize_t kernelx_ext4_inode_readat(
    struct ext4_inode_ref *inode,
    void *buf,
    size_t size,
    size_t offset
) {
    size_t inode_size;
	uint32_t unalg;
	uint32_t iblock_idx;
	uint32_t iblock_last;
	uint32_t block_size;

	ext4_fsblk_t fblock;
	ext4_fsblk_t fblock_start;
	uint32_t fblock_count;

	uint8_t *u8_buf = buf;
	int r;
	// struct ext4_inode_ref ref;

    size_t cnt = 0;

	if (size == 0) return 0;

	struct ext4_fs *const fs = inode->fs;
	struct ext4_sblock *const sb = &fs->sb;

    inode_size = ext4_inode_get_size(sb, inode->inode);

	block_size = ext4_sb_get_block_size(sb);
	// size = (size > inode_size - offset)
	// 	? ((size_t)(inode_size - offset)) : size;
    size = min(size, inode_size - offset);

	// iblock_idx = (uint32_t)((file->fpos) / block_size);
	// iblock_last = (uint32_t)((file->fpos + size) / block_size);
	// unalg = (file->fpos) % block_size;
    iblock_idx = (uint32_t)(offset / block_size);
	iblock_last = (uint32_t)((offset + size) / block_size);
	unalg = offset % block_size;

	/*If the size of symlink is smaller than 60 bytes*/
	bool softlink;
	// softlink = ext4_inode_is_type(sb, ref.inode, EXT4_INODE_MODE_SOFTLINK);
    softlink = ext4_inode_is_type(&fs->sb, inode->inode, EXT4_INODE_MODE_SOFTLINK);
	// if (softlink && file->fsize < sizeof(ref.inode->blocks)
	// 	     && !ext4_inode_get_blocks_count(sb, ref.inode)) {
    if (softlink && inode_size < sizeof(inode->inode->blocks)
         && !ext4_inode_get_blocks_count(sb, inode->inode)) {

		char *content = (char *)inode->inode->blocks;
		// if (file->fpos < file->fsize) {
		// 	size_t len = size;
		// 	if (unalg + size > (uint32_t)file->fsize)
		// 		len = (uint32_t)file->fsize - unalg;
		// 	memcpy(buf, content + unalg, len);
		// 	if (rcnt)
		// 		*rcnt = len;

		// }
        if (offset < inode_size) {
            size_t len = size;
            if (unalg + size > inode_size)
                len = inode_size - unalg;
            memcpy(buf, content + unalg, len);
            cnt = len;
        }

		// r = EOK;
        return cnt;
		// goto Finish;
	}

	if (unalg) {
		size_t len =  size;
		if (size > (block_size - unalg))
			len = block_size - unalg;

		// r = ext4_fs_get_inode_dblk_idx(&ref, iblock_idx, &fblock, true);
        r = ext4_fs_get_inode_dblk_idx(inode, iblock_idx, &fblock, true);
		if (r != EOK) {
            return -r;
			// goto Finish;
        }
		
        /* Do we get an unwritten range? */
		if (fblock != 0) {
			uint64_t off = fblock * block_size + unalg;
			// r = ext4_block_readbytes(file->mp->fs.bdev, off, u8_buf, len);
            r = ext4_block_readbytes(fs->bdev, off, u8_buf, len);
			if (r != EOK) {
                // goto Finish;
                return -r;
            }
		} else {
			/* Yes, we do. */
			memset(u8_buf, 0, len);
		}

		u8_buf += len;
		size -= len;
		// file->fpos += len;

		// if (rcnt)
		// 	*rcnt += len;
        cnt += len;

		iblock_idx++;
	}

	fblock_start = 0;
	fblock_count = 0;
	while (size >= block_size) {
		while (iblock_idx < iblock_last) {
			// r = ext4_fs_get_inode_dblk_idx(&ref, iblock_idx,
			// 			       &fblock, true);
            r = ext4_fs_get_inode_dblk_idx(inode, iblock_idx, &fblock, true);
			if (r != EOK) {
                return -r;
                // goto Finish;
            }
			
			iblock_idx++;

			if (!fblock_start)
				fblock_start = fblock;

			if ((fblock_start + fblock_count) != fblock)
				break;

			fblock_count++;
		}

		// r = ext4_blocks_get_direct(file->mp->fs.bdev, u8_buf, fblock_start,
		// 			   fblock_count);
        r = ext4_blocks_get_direct(fs->bdev, u8_buf, fblock_start,
                       fblock_count);
		if (r != EOK) {
            return -r;
            // goto Finish;
        }

		size -= block_size * fblock_count;
		u8_buf += block_size * fblock_count;
		// file->fpos += block_size * fblock_count;

		// if (rcnt)
		// 	*rcnt += block_size * fblock_count;
        cnt += block_size * fblock_count;

		fblock_start = fblock;
		fblock_count = 1;
	}

	if (size) {
		uint64_t off;
		// r = ext4_fs_get_inode_dblk_idx(&ref, iblock_idx, &fblock, true);
        r = ext4_fs_get_inode_dblk_idx(inode, iblock_idx, &fblock, true);
		if (r != EOK) {
            return -r;
            // goto Finish;
        }

		off = fblock * block_size;
		// r = ext4_block_readbytes(file->mp->fs.bdev, off, u8_buf, size);
        r = ext4_block_readbytes(fs->bdev, off, u8_buf, size);
		if (r != EOK) {
            return -r;
            // goto Finish;
        }

		// file->fpos += size;

		// if (rcnt)
		// 	*rcnt += size;
        cnt += size;
	}

    return cnt;

// Finish:
	// ext4_fs_put_inode_ref(&ref);
	// EXT4_MP_UNLOCK(file->mp);
	// return r;
}