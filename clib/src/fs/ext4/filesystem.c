#include "ext4_bcache.h"
#include "ext4_blockdev.h"
#include "ext4_errno.h"
#include "ext4_fs.h"
#include "ext4_super.h"

#include <stdbool.h>
#include <stddef.h>
#include <stdint.h>
#include <stdlib.h>
#include <string.h>

typedef struct BlockDevice {
    int (*open)(void *user);
    int (*bread)(void *user, void *buf, uint64_t block_id, uint32_t block_count);
    int (*bwrite)(void *user, const void *buf, uint64_t block_id, uint32_t block_count);
    int (*close)(void *user);
    void *user;
} BlockDevice;

static int open(struct ext4_blockdev *bd) {
    BlockDevice *d = (BlockDevice *)bd->bdif->p_user;
    return d->open(d->user);
}

static int bread(struct ext4_blockdev *bd, void *buf, uint64_t block, uint32_t count) {
    BlockDevice *d = (BlockDevice *)bd->bdif->p_user;
    return d->bread(d->user, buf, block, count);
}

static int bwrite(struct ext4_blockdev *bd, const void *buf, uint64_t block, uint32_t count) {
    BlockDevice *d = (BlockDevice *)bd->bdif->p_user;
    return d->bwrite(d->user, buf, block, count);
}

static int close(struct ext4_blockdev *bd) {
    BlockDevice *d = (BlockDevice *)bd->bdif->p_user;
    return d->close(d->user);
}

static int lock(struct ext4_blockdev *bd) {
    // Implement locking logic if needed
    (void)bd;
    return EOK;
}

static int unlock(struct ext4_blockdev *bd) {
    // Implement unlocking logic if needed
    (void)bd;
    return EOK;
}

const size_t BLOCK_SIZE = 512;

int kernelx_ext4_create_filesystem(
    uint32_t block_size,
    uint64_t block_count,
    uintptr_t f_open,
    uintptr_t f_bread,
    uintptr_t f_bwrite,
    uintptr_t f_close,
    void *user,
    struct ext4_fs **return_fs
) {
    struct ext4_blockdev *bd = (struct ext4_blockdev *)malloc(sizeof(struct ext4_blockdev));
    memset(bd, 0, sizeof(struct ext4_blockdev));
    
    BlockDevice *block_user = (BlockDevice *)malloc(sizeof(BlockDevice));
    *block_user = (BlockDevice){
        .open   = (int (*)(void *))f_open,
        .bread  = (int (*)(void *, void *, uint64_t, uint32_t))f_bread,
        .bwrite = (int (*)(void *, const void *, uint64_t, uint32_t))f_bwrite,
        .close  = (int (*)(void *))f_close,
        .user   = user
    };

    bd->bdif = (struct ext4_blockdev_iface *)malloc(sizeof(struct ext4_blockdev_iface));
    *bd->bdif = (struct ext4_blockdev_iface){
        .open       = open,
        .bread      = bread,
        .bwrite     = bwrite,
        .close      = close,
        .lock       = lock,
        .unlock     = unlock,
        .ph_bsize   = block_size,
        .ph_bcnt    = block_count,
        .ph_bbuf    = malloc(block_size),
        .ph_refctr  = 0,
        .bread_ctr  = 0,
        .bwrite_ctr = 0,
        .p_user     = block_user
    };

    bd->part_offset = 0;
    bd->part_size = block_count * block_size;

    int r;
	r = ext4_block_init(bd);
	if (r != EOK)
		return -r;

    struct ext4_fs *fs = (struct ext4_fs *)malloc(sizeof(struct ext4_fs));
    memset(fs, 0, sizeof(struct ext4_fs));

	r = ext4_fs_init(fs, bd, false);
	if (r != EOK) {
		ext4_block_fini(bd);
        
        free(bd->bdif->ph_bbuf);
        free(bd->bdif->p_user);
        free(bd->bdif);
        free(bd);
		
        return -r;
	}

    uint32_t bsize = ext4_sb_get_block_size(&fs->sb);
	ext4_block_set_lb_size(bd, bsize);

    struct ext4_bcache *bc = (struct ext4_bcache *)malloc(sizeof(struct ext4_bcache));
    memset(bc, 0, sizeof(struct ext4_bcache));

	r = ext4_bcache_init_dynamic(bc, CONFIG_BLOCK_DEV_CACHE_SIZE, bsize);
	if (r != EOK) {
		ext4_block_fini(bd);
        
        free(bc);
        free(bd->bdif->ph_bbuf);
        free(bd->bdif->p_user);
        free(bd->bdif);
        free(bd);
        free(fs);

        return -r;
	}

	if (bsize != bc->itemsize)
		return -ENOTSUP;

	/*Bind block cache to block device*/
	r = ext4_block_bind_bcache(bd, bc);
	if (r != EOK) {
		ext4_bcache_cleanup(bc);
		ext4_block_fini(bd);
		ext4_bcache_fini_dynamic(bc);
		
        free(bc);
        free(bd->bdif->ph_bbuf);
        free(bd->bdif->p_user);
        free(bd->bdif);
        free(bd);
        free(fs);
        
        return -r;
	}

	bd->fs = fs;
    
    *return_fs = fs;
	
    return EOK;
}

int kernelx_ext4_destroy_filesystem(
    struct ext4_fs *fs
) {
    if (!fs)
        return EINVAL;

    int r = ext4_fs_fini(fs);
    if (r != EOK)
        return -r;

    ext4_bcache_cleanup(fs->bdev->bc);
    ext4_bcache_fini_dynamic(fs->bdev->bc);

    r = ext4_block_fini(fs->bdev);
    if (r != EOK)
        return -r;

    free(fs->bdev->bdif->ph_bbuf);
    free(fs->bdev->bdif->p_user);
    free(fs->bdev->bdif);
    free(fs->bdev);
    
    free(fs);

    return EOK;
}
