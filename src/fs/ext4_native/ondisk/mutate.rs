use alloc::vec;
use alloc::vec::Vec;

use crate::kernel::errno::{Errno, SysResult};

use super::*;

const ROOT_EXTENT_MAX: u16 = ((INODE_BLOCK_ARRAY_SIZE - EXTENT_HEADER_SIZE) / EXTENT_ENTRY_SIZE) as u16;
const DEFAULT_EXTRA_ISIZE: u16 = 32;
const EXT_INIT_MAX_LEN: u16 = 32768;
const EXT4_FT_DIR: u8 = 2;

#[derive(Clone, Copy)]
struct BuiltExtentNode {
    first_lblk: u32,
    pblk: u64,
    depth: u16,
}

impl Context {
    pub fn alloc_block(&self) -> SysResult<u64> {
        for group in 0..self.groups_count {
            let mut gd = self.read_group_desc(group)?;
            if gd.free_blocks_count == 0 {
                continue;
            }

            let mut bitmap = self.read_block_bitmap(group)?;
            for bit in 0..self.blocks_per_group {
                let pblk = self
                    .group_first_block(group)?
                    .checked_add(bit as u64)
                    .ok_or_else(|| debug_errno("alloc_block: physical block calculation overflow", Errno::EINVAL))?;
                if pblk >= self.blocks_count || test_bit(&bitmap, bit) {
                    continue;
                }

                set_bit(&mut bitmap, bit);
                self.write_block_bitmap(group, &mut gd, &bitmap)?;

                gd.free_blocks_count = gd
                    .free_blocks_count
                    .checked_sub(1)
                    .ok_or_else(|| debug_errno("alloc_block: free_blocks_count underflow", Errno::EIO))?;
                self.write_group_desc(group, &mut gd)?;
                self.dec_superblock_free_blocks()?;

                let zero = vec![0u8; self.block_size as usize];
                self.write_fs_block(pblk, &zero)?;
                return Ok(pblk);
            }
        }

        ret_errno("alloc_block: no free data block", Errno::ENOSPC)
    }

    pub fn free_block(&self, pblk: u64) -> SysResult<()> {
        let (group, bit) = self.block_group_bit(pblk)?;
        let mut gd = self.read_group_desc(group)?;
        let mut bitmap = self.read_block_bitmap(group)?;

        if !test_bit(&bitmap, bit) {
            return ret_errno("free_block: block bitmap bit already clear", Errno::EIO);
        }

        clear_bit(&mut bitmap, bit);
        self.write_block_bitmap(group, &mut gd, &bitmap)?;

        gd.free_blocks_count = gd
            .free_blocks_count
            .checked_add(1)
            .ok_or_else(|| debug_errno("free_block: free_blocks_count overflow", Errno::EIO))?;
        self.write_group_desc(group, &mut gd)?;
        self.inc_superblock_free_blocks()
    }

    pub fn alloc_inode(&self, is_dir: bool) -> SysResult<u32> {
        for group in 0..self.groups_count {
            let mut gd = self.read_group_desc(group)?;
            if gd.free_inodes_count == 0 {
                continue;
            }

            let mut bitmap = self.read_inode_bitmap(group)?;
            for bit in 0..self.inodes_per_group {
                if test_bit(&bitmap, bit) {
                    continue;
                }

                set_bit(&mut bitmap, bit);
                self.write_inode_bitmap(group, &mut gd, &bitmap)?;

                gd.free_inodes_count = gd
                    .free_inodes_count
                    .checked_sub(1)
                    .ok_or_else(|| debug_errno("alloc_inode: free_inodes_count underflow", Errno::EIO))?;
                if is_dir {
                    gd.used_dirs_count = gd
                        .used_dirs_count
                        .checked_add(1)
                        .ok_or_else(|| debug_errno("alloc_inode: used_dirs_count overflow", Errno::EIO))?;
                }
                self.mark_inode_initialized(&mut gd, bit)?;
                self.write_group_desc(group, &mut gd)?;
                self.dec_superblock_free_inodes()?;

                let ino = group
                    .checked_mul(self.inodes_per_group)
                    .and_then(|v| v.checked_add(bit))
                    .and_then(|v| v.checked_add(1))
                    .ok_or_else(|| debug_errno("alloc_inode: inode number overflow", Errno::EINVAL))?;
                self.zero_inode_record(ino)?;
                return Ok(ino);
            }
        }

        ret_errno("alloc_inode: no free inode", Errno::ENOSPC)
    }

    pub fn free_inode_bit(&self, ino: u32, was_dir: bool) -> SysResult<()> {
        if ino == 0 {
            return ret_errno("free_inode_bit: inode number is zero", Errno::EINVAL);
        }

        let idx = ino - 1;
        let group = idx / self.inodes_per_group;
        let bit = idx % self.inodes_per_group;
        let mut gd = self.read_group_desc(group)?;
        let mut bitmap = self.read_inode_bitmap(group)?;

        if !test_bit(&bitmap, bit) {
            return ret_errno("free_inode_bit: inode bitmap bit already clear", Errno::EIO);
        }

        clear_bit(&mut bitmap, bit);
        self.write_inode_bitmap(group, &mut gd, &bitmap)?;

        gd.free_inodes_count = gd
            .free_inodes_count
            .checked_add(1)
            .ok_or_else(|| debug_errno("free_inode_bit: free_inodes_count overflow", Errno::EIO))?;
        if was_dir {
            gd.used_dirs_count = gd
                .used_dirs_count
                .checked_sub(1)
                .ok_or_else(|| debug_errno("free_inode_bit: used_dirs_count underflow", Errno::EIO))?;
        }
        self.write_group_desc(group, &mut gd)?;
        self.inc_superblock_free_inodes()
    }

    pub fn zero_inode_record(&self, ino: u32) -> SysResult<()> {
        let (table_block, off_in_block) = self.inode_location(ino)?;
        let mut block = self.read_fs_block(table_block)?;
        let inode_size = self.inode_size as usize;
        let end = off_in_block
            .checked_add(inode_size)
            .ok_or_else(|| debug_errno("zero_inode_record: inode range overflow", Errno::EINVAL))?;
        if end > block.len() {
            return ret_errno("zero_inode_record: inode range exceeds block boundary", Errno::EIO);
        }

        block[off_in_block..end].fill(0);
        self.write_fs_block(table_block, &block)
    }

    pub fn new_inode(&self, ino: u32, mode: u16, uid: u16, gid: u16, links: u16) -> SysResult<Ext4Inode> {
        let mut inode = Ext4Inode {
            raw: vec![0u8; self.inode_size as usize],
            ino,
            i_mode: mode,
            i_uid: uid,
            i_gid: gid,
            i_links_count: links,
            i_size: 0,
            i_blocks: 0,
            i_flags: Ext4InodeFlags::EXTENTS,
            i_generation: 0,
            i_file_acl: 0,
            i_extra_isize: self.default_extra_isize(),
        };
        self.init_empty_extent_root(&mut inode)?;
        Ok(inode)
    }

    pub fn init_empty_extent_root(&self, inode: &mut Ext4Inode) -> SysResult<()> {
        let i_block = inode.i_block_mut();
        i_block.fill(0);
        put_u16_le(i_block, 0, EXT4_EXTENT_MAGIC)?;
        put_u16_le(i_block, 2, 0)?;
        put_u16_le(i_block, 4, ROOT_EXTENT_MAX)?;
        put_u16_le(i_block, 6, 0)?;
        put_u32_le(i_block, 8, 0)
    }

    pub fn insert_extent_1blk(&self, ino: u32, inode: &mut Ext4Inode, lblk: u32, pblk: u64) -> SysResult<()> {
        if !inode.i_flags.contains(Ext4InodeFlags::EXTENTS) {
            return ret_errno("insert_extent_1blk: inode does not use extents", Errno::EOPNOTSUPP);
        }

        let root = self.parse_extent_root(inode)?;
        let tree_generation = root.header.eh_generation;
        let mut extents = Vec::new();
        let mut old_extent_blocks = Vec::new();
        self.collect_extent_leaves(ino, inode.i_generation, &root, &mut extents, &mut old_extent_blocks)?;
        let old_extents = extents.clone();

        insert_extent_leaf(&mut extents, lblk, pblk)?;
        self.rebuild_extent_tree(ino, inode, tree_generation, &extents, &old_extent_blocks)?;
        self.update_i_blocks_from_extent_delta(inode, &old_extents, old_extent_blocks.len(), &extents)
    }

    pub fn extent_tree_snapshot(&self, ino: u32, inode: &Ext4Inode) -> SysResult<(u32, Vec<ExtentLeaf>, Vec<u64>)> {
        if !inode.i_flags.contains(Ext4InodeFlags::EXTENTS) {
            return ret_errno("extent_tree_snapshot: inode does not use extents", Errno::EOPNOTSUPP);
        }

        let root = self.parse_extent_root(inode)?;
        let tree_generation = root.header.eh_generation;
        let mut extents = Vec::new();
        let mut old_extent_blocks = Vec::new();
        self.collect_extent_leaves(ino, inode.i_generation, &root, &mut extents, &mut old_extent_blocks)?;
        Ok((tree_generation, extents, old_extent_blocks))
    }

    pub fn replace_extent_tree(
        &self,
        ino: u32,
        inode: &mut Ext4Inode,
        tree_generation: u32,
        extents: &[ExtentLeaf],
        old_extent_blocks: &[u64],
    ) -> SysResult<()> {
        self.rebuild_extent_tree(ino, inode, tree_generation, extents, old_extent_blocks)
    }

    pub fn insert_extent_mapping(&self, extents: &mut Vec<ExtentLeaf>, lblk: u32, pblk: u64) -> SysResult<()> {
        insert_extent_leaf(extents, lblk, pblk)
    }

    pub fn update_i_blocks_from_extent_delta(
        &self,
        inode: &mut Ext4Inode,
        old_extents: &[ExtentLeaf],
        old_external_extent_blocks: usize,
        new_extents: &[ExtentLeaf],
    ) -> SysResult<()> {
        if !inode.i_flags.contains(Ext4InodeFlags::EXTENTS) {
            inode.i_blocks = 0;
            return Ok(());
        }

        let old_fs_blocks = extent_data_blocks(old_extents)?
            .checked_add(old_external_extent_blocks as u64)
            .ok_or_else(|| {
                debug_errno(
                    "update_i_blocks_from_extent_delta: old fs block count overflow",
                    Errno::EINVAL,
                )
            })?;
        let new_external_extent_blocks = self.external_extent_block_count_for_extent_count(new_extents.len())?;
        let new_fs_blocks = extent_data_blocks(new_extents)?
            .checked_add(new_external_extent_blocks)
            .ok_or_else(|| {
                debug_errno(
                    "update_i_blocks_from_extent_delta: new fs block count overflow",
                    Errno::EINVAL,
                )
            })?;
        let sectors_per_fs_block = (self.block_size / 512) as u64;

        if new_fs_blocks >= old_fs_blocks {
            let delta = new_fs_blocks
                .checked_sub(old_fs_blocks)
                .and_then(|blocks| blocks.checked_mul(sectors_per_fs_block))
                .ok_or_else(|| {
                    debug_errno(
                        "update_i_blocks_from_extent_delta: positive sector delta overflow",
                        Errno::EINVAL,
                    )
                })?;
            inode.i_blocks = inode
                .i_blocks
                .checked_add(delta)
                .ok_or_else(|| debug_errno("update_i_blocks_from_extent_delta: i_blocks overflow", Errno::EINVAL))?;
        } else {
            let delta = old_fs_blocks
                .checked_sub(new_fs_blocks)
                .and_then(|blocks| blocks.checked_mul(sectors_per_fs_block))
                .ok_or_else(|| {
                    debug_errno(
                        "update_i_blocks_from_extent_delta: negative sector delta overflow",
                        Errno::EINVAL,
                    )
                })?;
            inode.i_blocks = inode
                .i_blocks
                .checked_sub(delta)
                .ok_or_else(|| debug_errno("update_i_blocks_from_extent_delta: i_blocks underflow", Errno::EINVAL))?;
        }

        Ok(())
    }

    pub fn remove_extent_range(
        &self,
        ino: u32,
        inode: &mut Ext4Inode,
        first_lblk: u32,
        last_lblk_inclusive: u32,
    ) -> SysResult<()> {
        if !inode.i_flags.contains(Ext4InodeFlags::EXTENTS) {
            return ret_errno("remove_extent_range: inode does not use extents", Errno::EOPNOTSUPP);
        }
        if first_lblk > last_lblk_inclusive {
            return Ok(());
        }

        let root = self.parse_extent_root(inode)?;
        let tree_generation = root.header.eh_generation;
        let mut extents = Vec::new();
        let mut old_extent_blocks = Vec::new();
        self.collect_extent_leaves(ino, inode.i_generation, &root, &mut extents, &mut old_extent_blocks)?;

        let (new_extents, blocks_to_free) = remove_extent_leaves(&extents, first_lblk, last_lblk_inclusive)?;
        self.rebuild_extent_tree(ino, inode, tree_generation, &new_extents, &old_extent_blocks)?;

        for pblk in blocks_to_free {
            self.free_block(pblk)?;
        }

        self.update_i_blocks_from_extent_delta(inode, &extents, old_extent_blocks.len(), &new_extents)?;
        Ok(())
    }

    pub fn init_dir_block(&self, pblk: u64, dir_ino: u32, dir_generation: u32, parent_ino: u32) -> SysResult<()> {
        let limit = writable_dir_bytes(self.block_size as usize, self.metadata_csum)?;
        let dot_len = dirent_min_len(1)?;
        let dotdot_len = u16::try_from(limit)
            .map_err(|_| debug_errno("init_dir_block: directory block length does not fit u16", Errno::EINVAL))?
            .checked_sub(dot_len)
            .ok_or_else(|| {
                debug_errno(
                    "init_dir_block: directory block too small for dot entries",
                    Errno::EINVAL,
                )
            })?;
        if dotdot_len < DIR_ENTRY_HEADER_SIZE as u16 {
            return ret_errno(
                "init_dir_block: directory block too small for dotdot entry",
                Errno::EINVAL,
            );
        }

        let mut block = DirBlock {
            pblk,
            raw: Vec::new(),
            entries: vec![
                new_dir_entry(dir_ino, b".", EXT4_FT_DIR, dot_len)?,
                new_dir_entry(parent_ino, b"..", EXT4_FT_DIR, dotdot_len)?,
            ],
        };
        self.write_dir_block(dir_ino, dir_generation, &mut block)
    }

    pub fn insert_dirent(
        &self,
        dir_ino: u32,
        dir_inode: &mut Ext4Inode,
        name: &[u8],
        child_ino: u32,
        file_type: u8,
    ) -> SysResult<()> {
        if dir_inode.i_flags.contains(Ext4InodeFlags::INDEX) {
            return ret_errno("insert_dirent: htree indexed directory unsupported", Errno::EOPNOTSUPP);
        }
        if dir_inode.i_flags.contains(Ext4InodeFlags::INLINE_DATA) {
            return ret_errno("insert_dirent: inline_data directory unsupported", Errno::EOPNOTSUPP);
        }
        if !dir_inode.i_flags.contains(Ext4InodeFlags::EXTENTS) {
            return ret_errno("insert_dirent: non-extent directory unsupported", Errno::EOPNOTSUPP);
        }
        if name.is_empty() || name.len() > EXT4_MAX_NAME_LEN {
            return ret_errno("insert_dirent: invalid file name length", Errno::EINVAL);
        }

        let root = self.parse_extent_root(dir_inode)?;
        let total_blocks = dir_inode.i_size.div_ceil(self.block_size as u64) as u32;

        for lblk in 0..total_blocks {
            let Some(pblk) = lookup_lblk_mut(self, root.clone(), dir_ino, dir_inode.i_generation, lblk)? else {
                continue;
            };

            let mut dir_block = self.read_dir_block(dir_ino, dir_inode.i_generation, pblk)?;
            let old_block = dir_block.clone();
            if !insert_dirent_into_block(&mut dir_block, name, child_ino, file_type)? {
                continue;
            }

            self.write_dir_block(dir_ino, dir_inode.i_generation, &mut dir_block)?;
            if let Err(err) = self.write_inode(dir_inode) {
                let mut restore = old_block;
                let _ = self.write_dir_block(dir_ino, dir_inode.i_generation, &mut restore);
                return Err(err);
            }
            return Ok(());
        }

        let original = dir_inode.clone();
        let new_pblk = self.alloc_block()?;
        let result = (|| -> SysResult<()> {
            self.insert_extent_1blk(dir_ino, dir_inode, total_blocks, new_pblk)?;
            dir_inode.i_size = dir_inode
                .i_size
                .checked_add(self.block_size as u64)
                .ok_or_else(|| debug_errno("insert_dirent: directory size overflow", Errno::EINVAL))?;
            self.write_inode(dir_inode)?;

            let limit = writable_dir_bytes(self.block_size as usize, self.metadata_csum)?;
            let mut block = DirBlock {
                pblk: new_pblk,
                raw: Vec::new(),
                entries: vec![new_dir_entry(child_ino, name, file_type, limit as u16)?],
            };
            self.write_dir_block(dir_ino, dir_inode.i_generation, &mut block)
        })();

        if let Err(err) = result {
            let mut restore = original.clone();
            let _ = self.write_inode(&mut restore);
            let _ = self.free_block(new_pblk);
            *dir_inode = original;
            return Err(err);
        }

        Ok(())
    }

    pub fn remove_dirent(&self, dir_ino: u32, dir_inode: &mut Ext4Inode, name: &[u8]) -> SysResult<(u32, u8)> {
        if dir_inode.i_flags.contains(Ext4InodeFlags::INDEX) {
            return ret_errno("remove_dirent: htree indexed directory unsupported", Errno::EOPNOTSUPP);
        }
        if dir_inode.i_flags.contains(Ext4InodeFlags::INLINE_DATA) {
            return ret_errno("remove_dirent: inline_data directory unsupported", Errno::EOPNOTSUPP);
        }
        if !dir_inode.i_flags.contains(Ext4InodeFlags::EXTENTS) {
            return ret_errno("remove_dirent: non-extent directory unsupported", Errno::EOPNOTSUPP);
        }
        if name.is_empty() || name.len() > EXT4_MAX_NAME_LEN {
            return ret_errno("remove_dirent: invalid file name length", Errno::EINVAL);
        }

        let root = self.parse_extent_root(dir_inode)?;
        let total_blocks = dir_inode.i_size.div_ceil(self.block_size as u64) as u32;

        for lblk in 0..total_blocks {
            let Some(pblk) = lookup_lblk_mut(self, root.clone(), dir_ino, dir_inode.i_generation, lblk)? else {
                continue;
            };

            let mut dir_block = self.read_dir_block(dir_ino, dir_inode.i_generation, pblk)?;
            let Some((child_ino, file_type)) = remove_dirent_from_block(&mut dir_block, name)? else {
                continue;
            };
            self.write_dir_block(dir_ino, dir_inode.i_generation, &mut dir_block)?;
            return Ok((child_ino, file_type));
        }

        ret_errno("remove_dirent: name not found", Errno::ENOENT)
    }

    pub fn patch_dotdot(&self, dir_ino: u32, dir_inode: &Ext4Inode, parent_ino: u32) -> SysResult<()> {
        if dir_inode.i_flags.contains(Ext4InodeFlags::INDEX) {
            return ret_errno("patch_dotdot: htree indexed directory unsupported", Errno::EOPNOTSUPP);
        }
        if dir_inode.i_flags.contains(Ext4InodeFlags::INLINE_DATA) {
            return ret_errno("patch_dotdot: inline_data directory unsupported", Errno::EOPNOTSUPP);
        }
        if !dir_inode.i_flags.contains(Ext4InodeFlags::EXTENTS) {
            return ret_errno("patch_dotdot: non-extent directory unsupported", Errno::EOPNOTSUPP);
        }

        let root = self.parse_extent_root(dir_inode)?;
        let pblk = lookup_lblk_mut(self, root, dir_ino, dir_inode.i_generation, 0)?
            .ok_or_else(|| debug_errno("patch_dotdot: missing directory data block", Errno::EIO))?;
        let mut dir_block = self.read_dir_block(dir_ino, dir_inode.i_generation, pblk)?;

        for entry in &mut dir_block.entries {
            if entry.inode != 0 && entry.name_slice() == b".." {
                entry.inode = parent_ino;
                self.write_dir_block(dir_ino, dir_inode.i_generation, &mut dir_block)?;
                return Ok(());
            }
        }

        ret_errno("patch_dotdot: missing '..' entry", Errno::EIO)
    }

    fn encode_extent_root(&self, inode: &mut Ext4Inode, root: &mut ExtentBlock) -> SysResult<()> {
        let i_block = inode.i_block_mut();
        i_block.fill(0);
        put_u16_le(i_block, 0, root.header.eh_magic)?;
        put_u16_le(i_block, 2, root.header.eh_entries)?;
        put_u16_le(i_block, 4, root.header.eh_max)?;
        put_u16_le(i_block, 6, root.header.eh_depth)?;
        put_u32_le(i_block, 8, root.header.eh_generation)?;

        if root.header.eh_depth == 0 {
            for (i, leaf) in root.leaf.iter().enumerate() {
                let off = EXTENT_HEADER_SIZE + i * EXTENT_ENTRY_SIZE;
                put_u32_le(i_block, off, leaf.ee_block)?;
                put_u16_le(i_block, off + 4, leaf.ee_len_raw)?;
                put_u16_le(i_block, off + 6, (leaf.ee_start >> 32) as u16)?;
                put_u32_le(i_block, off + 8, leaf.ee_start as u32)?;
            }
        } else {
            for (i, idx) in root.idx.iter().enumerate() {
                let off = EXTENT_HEADER_SIZE + i * EXTENT_ENTRY_SIZE;
                put_u32_le(i_block, off, idx.ei_block)?;
                put_u32_le(i_block, off + 4, idx.ei_leaf as u32)?;
                put_u16_le(i_block, off + 8, (idx.ei_leaf >> 32) as u16)?;
                put_u16_le(i_block, off + 10, 0)?;
            }
        }

        Ok(())
    }

    fn rebuild_extent_tree(
        &self,
        ino: u32,
        inode: &mut Ext4Inode,
        tree_generation: u32,
        extents: &[ExtentLeaf],
        old_extent_blocks: &[u64],
    ) -> SysResult<()> {
        if extents.is_empty() {
            self.init_empty_extent_root(inode)?;
            self.write_inode(inode)?;
            for &pblk in old_extent_blocks {
                self.free_block(pblk)?;
            }
            return Ok(());
        }

        if extents.len() <= ROOT_EXTENT_MAX as usize {
            let mut root = ExtentBlock {
                pblk: 0,
                raw: Vec::new(),
                header: ExtentHeader {
                    eh_magic: EXT4_EXTENT_MAGIC,
                    eh_entries: extents.len() as u16,
                    eh_max: ROOT_EXTENT_MAX,
                    eh_depth: 0,
                    eh_generation: tree_generation,
                },
                idx: Vec::new(),
                leaf: extents.to_vec(),
                tail_checksum: None,
            };
            self.encode_extent_root(inode, &mut root)?;
            self.write_inode(inode)?;
            for &pblk in old_extent_blocks {
                self.free_block(pblk)?;
            }
            return Ok(());
        }

        let block_max = self.extent_block_max_entries()?;
        let block_max_usize = block_max as usize;
        let mut reused = 0usize;
        let mut newly_allocated = Vec::new();

        let mut nodes = Vec::new();
        for chunk in extents.chunks(block_max_usize) {
            let pblk = self.next_extent_tree_block(old_extent_blocks, &mut reused, &mut newly_allocated)?;
            let mut block = self.make_extent_block(pblk, 0, tree_generation, chunk, &[])?;
            self.write_extent_block(ino, inode.i_generation, &mut block)?;
            nodes.push(BuiltExtentNode {
                first_lblk: chunk[0].ee_block,
                pblk,
                depth: 0,
            });
        }

        while nodes.len() > ROOT_EXTENT_MAX as usize {
            let child_depth = nodes[0].depth;
            let mut parents = Vec::new();
            for chunk in nodes.chunks(block_max_usize) {
                let pblk = self.next_extent_tree_block(old_extent_blocks, &mut reused, &mut newly_allocated)?;
                let mut idx = Vec::with_capacity(chunk.len());
                for child in chunk {
                    idx.push(ExtentIdx {
                        ei_block: child.first_lblk,
                        ei_leaf: child.pblk,
                    });
                }
                let mut block = self.make_extent_block(pblk, child_depth + 1, tree_generation, &[], &idx)?;
                self.write_extent_block(ino, inode.i_generation, &mut block)?;
                parents.push(BuiltExtentNode {
                    first_lblk: idx[0].ei_block,
                    pblk,
                    depth: child_depth + 1,
                });
            }
            nodes = parents;
        }

        let root_depth = nodes[0]
            .depth
            .checked_add(1)
            .ok_or_else(|| debug_errno("rebuild_extent_tree: root depth overflow", Errno::EINVAL))?;
        let mut root = ExtentBlock {
            pblk: 0,
            raw: Vec::new(),
            header: ExtentHeader {
                eh_magic: EXT4_EXTENT_MAGIC,
                eh_entries: nodes.len() as u16,
                eh_max: ROOT_EXTENT_MAX,
                eh_depth: root_depth,
                eh_generation: tree_generation,
            },
            idx: nodes
                .iter()
                .map(|node| ExtentIdx {
                    ei_block: node.first_lblk,
                    ei_leaf: node.pblk,
                })
                .collect(),
            leaf: Vec::new(),
            tail_checksum: None,
        };
        self.encode_extent_root(inode, &mut root)?;
        self.write_inode(inode)?;

        for &pblk in &old_extent_blocks[reused..] {
            self.free_block(pblk)?;
        }

        Ok(())
    }

    fn collect_extent_leaves(
        &self,
        ino: u32,
        generation: u32,
        block: &ExtentBlock,
        extents: &mut Vec<ExtentLeaf>,
        extent_blocks: &mut Vec<u64>,
    ) -> SysResult<()> {
        if block.header.eh_depth == 0 {
            extents.extend_from_slice(&block.leaf);
            return Ok(());
        }

        for idx in &block.idx {
            extent_blocks.push(idx.ei_leaf);
            let child = self.read_extent_block(ino, generation, idx.ei_leaf)?;
            self.collect_extent_leaves(ino, generation, &child, extents, extent_blocks)?;
        }

        Ok(())
    }

    fn next_extent_tree_block(
        &self,
        old_extent_blocks: &[u64],
        reused: &mut usize,
        newly_allocated: &mut Vec<u64>,
    ) -> SysResult<u64> {
        if let Some(&pblk) = old_extent_blocks.get(*reused) {
            *reused += 1;
            Ok(pblk)
        } else {
            let pblk = self.alloc_block()?;
            newly_allocated.push(pblk);
            Ok(pblk)
        }
    }

    fn make_extent_block(
        &self,
        pblk: u64,
        depth: u16,
        tree_generation: u32,
        leaf: &[ExtentLeaf],
        idx: &[ExtentIdx],
    ) -> SysResult<ExtentBlock> {
        let eh_max = self.extent_block_max_entries()?;
        let entries = if depth == 0 { leaf.len() } else { idx.len() };
        if entries > eh_max as usize {
            return ret_errno("make_extent_block: extent block entries exceed eh_max", Errno::EINVAL);
        }

        Ok(ExtentBlock {
            pblk,
            raw: vec![0u8; self.block_size as usize],
            header: ExtentHeader {
                eh_magic: EXT4_EXTENT_MAGIC,
                eh_entries: entries as u16,
                eh_max,
                eh_depth: depth,
                eh_generation: tree_generation,
            },
            idx: idx.to_vec(),
            leaf: leaf.to_vec(),
            tail_checksum: None,
        })
    }

    fn extent_block_max_entries(&self) -> SysResult<u16> {
        let usable = (self.block_size as usize)
            .checked_sub(EXTENT_HEADER_SIZE)
            .ok_or_else(|| {
                debug_errno(
                    "extent_block_max_entries: block smaller than extent header",
                    Errno::EINVAL,
                )
            })?;
        let max = usable / EXTENT_ENTRY_SIZE;
        if max == 0 {
            return ret_errno(
                "extent_block_max_entries: extent block cannot hold any entry",
                Errno::EINVAL,
            );
        }
        u16::try_from(max).map_err(|_| debug_errno("extent_block_max_entries: eh_max does not fit u16", Errno::EINVAL))
    }

    fn group_first_block(&self, group: u32) -> SysResult<u64> {
        (group as u64)
            .checked_mul(self.blocks_per_group as u64)
            .and_then(|v| v.checked_add(self.first_data_block as u64))
            .ok_or_else(|| debug_errno("group_first_block: block number overflow", Errno::EINVAL))
    }

    fn block_group_bit(&self, pblk: u64) -> SysResult<(u32, u32)> {
        let first = self.first_data_block as u64;
        if pblk < first {
            return ret_errno("block_group_bit: block is before first_data_block", Errno::EINVAL);
        }

        let rel = pblk - first;
        let group = rel / self.blocks_per_group as u64;
        let bit = rel % self.blocks_per_group as u64;
        if group >= self.groups_count as u64 {
            return ret_errno("block_group_bit: block group out of range", Errno::EINVAL);
        }
        Ok((group as u32, bit as u32))
    }

    fn dec_superblock_free_blocks(&self) -> SysResult<()> {
        let mut sb = self.read_superblock()?;
        let free = sb
            .free_blocks_count()?
            .checked_sub(1)
            .ok_or_else(|| debug_errno("dec_superblock_free_blocks: underflow", Errno::EIO))?;
        sb.set_free_blocks_count(free)?;
        self.write_superblock(&mut sb)
    }

    fn inc_superblock_free_blocks(&self) -> SysResult<()> {
        let mut sb = self.read_superblock()?;
        let free = sb
            .free_blocks_count()?
            .checked_add(1)
            .ok_or_else(|| debug_errno("inc_superblock_free_blocks: overflow", Errno::EIO))?;
        sb.set_free_blocks_count(free)?;
        self.write_superblock(&mut sb)
    }

    fn dec_superblock_free_inodes(&self) -> SysResult<()> {
        let mut sb = self.read_superblock()?;
        let free = sb
            .free_inodes_count()?
            .checked_sub(1)
            .ok_or_else(|| debug_errno("dec_superblock_free_inodes: underflow", Errno::EIO))?;
        sb.set_free_inodes_count(free)?;
        self.write_superblock(&mut sb)
    }

    fn inc_superblock_free_inodes(&self) -> SysResult<()> {
        let mut sb = self.read_superblock()?;
        let free = sb
            .free_inodes_count()?
            .checked_add(1)
            .ok_or_else(|| debug_errno("inc_superblock_free_inodes: overflow", Errno::EIO))?;
        sb.set_free_inodes_count(free)?;
        self.write_superblock(&mut sb)
    }

    fn default_extra_isize(&self) -> u16 {
        if self.inode_size <= 128 {
            0
        } else {
            (self.inode_size - 128).min(DEFAULT_EXTRA_ISIZE)
        }
    }

    fn mark_inode_initialized(&self, gd: &mut Ext4GroupDesc, bit: u32) -> SysResult<()> {
        let initialized = self.inodes_per_group.checked_sub(gd.itable_unused).ok_or_else(|| {
            debug_errno(
                "mark_inode_initialized: itable_unused exceeds inodes_per_group",
                Errno::EIO,
            )
        })?;
        let new_initialized = bit
            .checked_add(1)
            .ok_or_else(|| debug_errno("mark_inode_initialized: inode index overflow", Errno::EINVAL))?;
        if new_initialized > initialized {
            gd.itable_unused = self
                .inodes_per_group
                .checked_sub(new_initialized)
                .ok_or_else(|| debug_errno("mark_inode_initialized: initialized range exceeds group", Errno::EIO))?;
        }
        gd.flags &= !EXT4_BG_INODE_UNINIT;
        Ok(())
    }

    fn external_extent_block_count_for_extent_count(&self, extent_count: usize) -> SysResult<u64> {
        if extent_count <= ROOT_EXTENT_MAX as usize {
            return Ok(0);
        }

        let block_max = self.extent_block_max_entries()? as usize;
        let mut nodes = extent_count.div_ceil(block_max) as u64;
        let mut total = nodes;

        while nodes > ROOT_EXTENT_MAX as u64 {
            nodes = nodes.div_ceil(block_max as u64);
            total = total.checked_add(nodes).ok_or_else(|| {
                debug_errno(
                    "external_extent_block_count_for_extent_count: node count overflow",
                    Errno::EINVAL,
                )
            })?;
        }

        Ok(total)
    }
}

fn extent_data_blocks(extents: &[ExtentLeaf]) -> SysResult<u64> {
    let mut total = 0u64;
    for leaf in extents {
        total = total
            .checked_add(extent_len_u32(leaf.ee_len_raw) as u64)
            .ok_or_else(|| debug_errno("extent_data_blocks: block count overflow", Errno::EINVAL))?;
    }
    Ok(total)
}

fn lookup_lblk_mut(
    context: &Context,
    root: ExtentBlock,
    ino: u32,
    generation: u32,
    lblk: u32,
) -> SysResult<Option<u64>> {
    let mut cur = root;
    loop {
        if cur.header.eh_depth == 0 {
            return Ok(find_in_leaves_mut(&cur.leaf, lblk));
        }

        let mut chosen_leaf: Option<u64> = None;
        for idx in &cur.idx {
            if idx.ei_block <= lblk {
                chosen_leaf = Some(idx.ei_leaf);
            } else {
                break;
            }
        }
        let next = chosen_leaf
            .or_else(|| cur.idx.first().map(|idx| idx.ei_leaf))
            .ok_or_else(|| debug_errno("lookup_lblk_mut: extent index lookup failed", Errno::EIO))?;
        cur = context.read_extent_block(ino, generation, next)?;
    }
}

fn find_in_leaves_mut(leaves: &[ExtentLeaf], lblk: u32) -> Option<u64> {
    for leaf in leaves {
        let len = extent_len_u32(leaf.ee_len_raw);
        if lblk < leaf.ee_block {
            return None;
        }
        let end = leaf.ee_block.checked_add(len)?;
        if lblk < end {
            return Some(leaf.ee_start + (lblk - leaf.ee_block) as u64);
        }
    }
    None
}

fn extent_len_u32(raw: u16) -> u32 {
    if raw <= EXT_INIT_MAX_LEN {
        raw as u32
    } else {
        (raw - EXT_INIT_MAX_LEN) as u32
    }
}

fn insert_extent_leaf(extents: &mut Vec<ExtentLeaf>, lblk: u32, pblk: u64) -> SysResult<()> {
    validate_extent_leaves(extents, "insert_extent_1blk")?;

    let pos = extents
        .iter()
        .position(|leaf| leaf.ee_block > lblk)
        .unwrap_or(extents.len());

    if pos > 0 {
        let prev = extents[pos - 1];
        let prev_end = prev
            .ee_block
            .checked_add(prev.ee_len_raw as u32)
            .ok_or_else(|| debug_errno("insert_extent_leaf: previous extent end overflow", Errno::EINVAL))?;
        if lblk < prev_end {
            return ret_errno("insert_extent_1blk: logical block already mapped", Errno::EEXIST);
        }
    }
    if pos < extents.len() && lblk == extents[pos].ee_block {
        return ret_errno("insert_extent_1blk: logical block already mapped", Errno::EEXIST);
    }

    extents.insert(
        pos,
        ExtentLeaf {
            ee_block: lblk,
            ee_len_raw: 1,
            ee_start: pblk,
        },
    );
    merge_extent_leaves(extents)
}

fn remove_extent_leaves(
    extents: &[ExtentLeaf],
    first_lblk: u32,
    last_lblk_inclusive: u32,
) -> SysResult<(Vec<ExtentLeaf>, Vec<u64>)> {
    validate_extent_leaves(extents, "remove_extent_range")?;

    let first = first_lblk as u64;
    let last_exclusive = (last_lblk_inclusive as u64)
        .checked_add(1)
        .ok_or_else(|| debug_errno("remove_extent_leaves: last logical block overflow", Errno::EINVAL))?;

    let mut kept = Vec::with_capacity(extents.len() + 1);
    let mut blocks_to_free = Vec::new();

    for leaf in extents {
        let len = leaf.ee_len_raw as u64;
        let leaf_block = leaf.ee_block as u64;
        let leaf_end = leaf_block
            .checked_add(len)
            .ok_or_else(|| debug_errno("remove_extent_leaves: extent end overflow", Errno::EINVAL))?;

        if leaf_end <= first || leaf_block >= last_exclusive {
            kept.push(*leaf);
            continue;
        }

        let ov_start = leaf_block.max(first);
        let ov_end = leaf_end.min(last_exclusive);

        if leaf_block < ov_start {
            kept.push(ExtentLeaf {
                ee_block: leaf.ee_block,
                ee_len_raw: (ov_start - leaf_block) as u16,
                ee_start: leaf.ee_start,
            });
        }

        let mut cur = ov_start;
        while cur < ov_end {
            let off = cur - leaf_block;
            blocks_to_free.push(leaf.ee_start + off);
            cur += 1;
        }

        if ov_end < leaf_end {
            let keep_len = (leaf_end - ov_end) as u16;
            let off = ov_end - leaf_block;
            kept.push(ExtentLeaf {
                ee_block: ov_end as u32,
                ee_len_raw: keep_len,
                ee_start: leaf.ee_start + off,
            });
        }
    }

    Ok((kept, blocks_to_free))
}

fn validate_extent_leaves(extents: &[ExtentLeaf], op: &str) -> SysResult<()> {
    let mut prev_end: Option<u32> = None;

    for leaf in extents {
        if leaf.ee_len_raw == 0 {
            return ret_errno("validate_extent_leaves: zero-length extent", Errno::EIO);
        }
        if leaf.ee_len_raw > EXT_INIT_MAX_LEN {
            return ret_errno(
                match op {
                    "insert_extent_1blk" => "insert_extent_1blk: uninitialized extent unsupported",
                    "remove_extent_range" => "remove_extent_range: uninitialized extent unsupported",
                    _ => "validate_extent_leaves: uninitialized extent unsupported",
                },
                Errno::EOPNOTSUPP,
            );
        }

        if let Some(end) = prev_end {
            if leaf.ee_block < end {
                return ret_errno("validate_extent_leaves: overlapping extents", Errno::EIO);
            }
        }

        prev_end = Some(
            leaf.ee_block
                .checked_add(leaf.ee_len_raw as u32)
                .ok_or_else(|| debug_errno("validate_extent_leaves: extent end overflow", Errno::EINVAL))?,
        );
    }

    Ok(())
}

fn merge_extent_leaves(extents: &mut Vec<ExtentLeaf>) -> SysResult<()> {
    let mut i = 0;
    while i + 1 < extents.len() {
        let left = extents[i];
        let right = extents[i + 1];

        let left_len = left.ee_len_raw as u32;
        let left_end = left
            .ee_block
            .checked_add(left_len)
            .ok_or_else(|| debug_errno("merge_extent_leaves: extent end overflow", Errno::EINVAL))?;
        if left_end > right.ee_block {
            return ret_errno("merge_extent_leaves: overlapping extents", Errno::EIO);
        }

        let left_pend = left
            .ee_start
            .checked_add(left_len as u64)
            .ok_or_else(|| debug_errno("merge_extent_leaves: physical end overflow", Errno::EINVAL))?;
        let merged_len = left_len
            .checked_add(right.ee_len_raw as u32)
            .ok_or_else(|| debug_errno("merge_extent_leaves: merged extent length overflow", Errno::EINVAL))?;

        if left_end == right.ee_block && left_pend == right.ee_start && merged_len <= EXT_INIT_MAX_LEN as u32 {
            extents[i].ee_len_raw = merged_len as u16;
            extents.remove(i + 1);
            continue;
        }

        i += 1;
    }

    Ok(())
}

fn dirent_min_len(name_len: usize) -> SysResult<u16> {
    let raw = DIR_ENTRY_HEADER_SIZE
        .checked_add(name_len)
        .ok_or_else(|| debug_errno("dirent_min_len: name length overflow", Errno::EINVAL))?;
    let aligned = (raw + 3) & !3;
    u16::try_from(aligned).map_err(|_| debug_errno("dirent_min_len: record length does not fit u16", Errno::EINVAL))
}

fn writable_dir_bytes(block_size: usize, metadata_csum: bool) -> SysResult<usize> {
    if metadata_csum {
        block_size
            .checked_sub(DIR_ENTRY_TAIL_SIZE)
            .ok_or_else(|| debug_errno("writable_dir_bytes: block smaller than dir tail", Errno::EINVAL))
    } else {
        Ok(block_size)
    }
}

fn new_dir_entry(inode: u32, name: &[u8], file_type: u8, rec_len: u16) -> SysResult<DirEntry2> {
    if name.len() > EXT4_MAX_NAME_LEN {
        return ret_errno("new_dir_entry: file name is too long", Errno::EINVAL);
    }

    let mut name_buf = [0u8; EXT4_MAX_NAME_LEN];
    name_buf[..name.len()].copy_from_slice(name);
    Ok(DirEntry2 {
        inode,
        rec_len,
        name_len: name.len() as u8,
        file_type,
        name: name_buf,
        entry_off: 0,
    })
}

fn unused_dir_entry(rec_len: u16) -> DirEntry2 {
    DirEntry2 {
        inode: 0,
        rec_len,
        name_len: 0,
        file_type: 0,
        name: [0u8; EXT4_MAX_NAME_LEN],
        entry_off: 0,
    }
}

fn insert_dirent_into_block(block: &mut DirBlock, name: &[u8], child_ino: u32, file_type: u8) -> SysResult<bool> {
    let need = dirent_min_len(name.len())?;

    for idx in 0..block.entries.len() {
        let entry = &block.entries[idx];
        if entry.inode == 0 {
            if entry.rec_len < need {
                continue;
            }

            let remain = entry.rec_len - need;
            if remain >= DIR_ENTRY_HEADER_SIZE as u16 {
                block.entries[idx] = new_dir_entry(child_ino, name, file_type, need)?;
                block.entries.insert(idx + 1, unused_dir_entry(remain));
            } else {
                block.entries[idx] = new_dir_entry(child_ino, name, file_type, entry.rec_len)?;
            }
            return Ok(true);
        }

        let actual = dirent_min_len(entry.name_len as usize)?;
        if entry.rec_len < actual + need {
            continue;
        }

        let new_rec_len = entry.rec_len - actual;
        block.entries[idx].rec_len = actual;
        block
            .entries
            .insert(idx + 1, new_dir_entry(child_ino, name, file_type, new_rec_len)?);
        return Ok(true);
    }

    Ok(false)
}

fn remove_dirent_from_block(block: &mut DirBlock, name: &[u8]) -> SysResult<Option<(u32, u8)>> {
    for idx in 0..block.entries.len() {
        let entry = &block.entries[idx];
        if entry.inode == 0 || entry.name_slice() != name {
            continue;
        }

        let removed = (entry.inode, entry.file_type);
        let removed_len = entry.rec_len;

        if idx > 0 {
            block.entries[idx - 1].rec_len = block.entries[idx - 1]
                .rec_len
                .checked_add(removed_len)
                .ok_or_else(|| debug_errno("remove_dirent_from_block: rec_len overflow", Errno::EINVAL))?;
            block.entries.remove(idx);
        } else if block.entries.len() > 1 && block.entries[1].inode == 0 {
            block.entries[1].rec_len = block.entries[1]
                .rec_len
                .checked_add(removed_len)
                .ok_or_else(|| debug_errno("remove_dirent_from_block: rec_len overflow", Errno::EINVAL))?;
            block.entries.remove(0);
        } else {
            block.entries[0] = unused_dir_entry(removed_len);
        }

        return Ok(Some(removed));
    }

    Ok(None)
}
