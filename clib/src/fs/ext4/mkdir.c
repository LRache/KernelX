// #include "ext4.h"
// #include "ext4_dir.h"
// #include "ext4_dir_idx.h"
// #include "ext4_fs.h"
// #include "ext4_inode.h"

// #include <string.h>

// int kernelx_ext4_inode_mkdir(
//     struct ext4_inode_ref *parent,
//     const char *name,
//     uint32_t mode
// ) {
//     struct ext4_fs *fs = parent->fs;
//     struct ext4_inode_ref child_ref;
//     int r;

// 	struct ext4_dir_search_result result;
// 	r = ext4_dir_find_entry(&result, parent, name, strlen(name));
// 	if (r == EOK) {
// 		ext4_dir_destroy_result(parent, &result);
// 		return EEXIST;
// 	} else if (r != ENOENT) {
// 		return r;
// 	}
    
//     r = ext4_fs_alloc_inode(fs, &child_ref, EXT4_DE_DIR);
//     if (r != EOK) {
//         return r;
//     }

//     ext4_fs_inode_blocks_init(fs, &child_ref);

//     r = ext4_dir_add_entry(parent, name, strlen(name), &child_ref);
//     if (r != EOK) {
//         ext4_fs_free_inode(&child_ref);
//         return r;
//     }

// #if CONFIG_DIR_INDEX_ENABLE
// 	/* Initialize directory index if supported */
// 	if (ext4_sb_feature_com(&fs->sb, EXT4_FCOM_DIR_INDEX)) {
// 		r = ext4_dir_dx_init(&child_ref, parent);
// 		if (r != EOK)
// 				return r;

// 		ext4_inode_set_flag(child_ref.inode, EXT4_INODE_FLAG_INDEX);
// 		child_ref.dirty = true;
// 	} else
// #endif
// 	{
// 		r = ext4_dir_add_entry(&child_ref, ".", strlen("."), &child_ref);
// 		if (r != EOK) {
// 			ext4_dir_remove_entry(parent, name, strlen(name));
// 			return r;
// 		}

// 		r = ext4_dir_add_entry(&child_ref, "..", strlen(".."), parent);
// 		if (r != EOK) {
// 			ext4_dir_remove_entry(parent, name, strlen(name));
// 			ext4_dir_remove_entry(&child_ref, ".", strlen("."));
// 			return r;
// 		}
// 	}

// 	/*New empty directory. Two links (. and ..) */
// 	ext4_inode_set_links_cnt(child_ref.inode, 2);
// 	ext4_fs_inode_links_count_inc(parent);
// 	child_ref.dirty = true;
// 	parent->dirty = true;

// 	ext4_fs_put_inode_ref(&child_ref);
	
//     return r;
// }
