use alloc::vec;
use alloc::vec::Vec;
use core::cmp;

use crate::kernel::errno::{Errno, SysResult};
use crate::klib::crc::crc32c_update;

use super::*;

const ROOT_EXTENT_MAX: u16 = ((INODE_BLOCK_ARRAY_SIZE - EXTENT_HEADER_SIZE) / EXTENT_ENTRY_SIZE) as u16;
const DEFAULT_EXTRA_ISIZE: u16 = 32;
const EXT_INIT_MAX_LEN: u16 = 32768;
const DX_ROOT_INFO_OFF: usize = 24;
const DX_ROOT_ENTRY_OFF: usize = 32;
const DX_NODE_ENTRY_OFF: usize = 8;
const DX_ENTRY_SIZE: usize = 8;
const EXT4_SUPERBLOCK_FLAGS_UNSIGNED_HASH: u32 = 0x0002;

#[derive(Clone, Copy)]
struct BuiltExtentNode {
    first_lblk: u32,
    pblk: u64,
    depth: u16,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum HTreeHashVersion {
    Legacy = 0,
    HalfMd4 = 1,
    Tea = 2,
    LegacyUnsigned = 3,
    HalfMd4Unsigned = 4,
    TeaUnsigned = 5,
}

impl HTreeHashVersion {
    fn from_raw(version: u8, superblock_flags: u32) -> SysResult<Self> {
        let version =
            if version <= HTreeHashVersion::Tea as u8 && superblock_flags & EXT4_SUPERBLOCK_FLAGS_UNSIGNED_HASH != 0 {
                version + 3
            } else {
                version
            };
        match version {
            0 => Ok(Self::Legacy),
            1 => Ok(Self::HalfMd4),
            2 => Ok(Self::Tea),
            3 => Ok(Self::LegacyUnsigned),
            4 => Ok(Self::HalfMd4Unsigned),
            5 => Ok(Self::TeaUnsigned),
            _ => Err(Errno::EOPNOTSUPP),
        }
    }

    fn unsigned(self) -> bool {
        matches!(self, Self::LegacyUnsigned | Self::HalfMd4Unsigned | Self::TeaUnsigned)
    }
}

#[derive(Clone, Copy)]
struct HTreeNode {
    entry_offset: usize,
    limit: u16,
    count: u16,
}

struct HTreeLeafPath {
    index_pblk: u64,
    index_raw: Vec<u8>,
    index_node: HTreeNode,
    index_position: usize,
    leaf_pblk: u64,
}

struct HashedDirectoryEntry {
    hash: u32,
    ino: u32,
    name: Vec<u8>,
    file_type: u8,
    rec_len: u16,
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
            i_atime: 0,
            i_ctime: 0,
            i_mtime: 0,
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
                new_dir_entry(dir_ino, b".", Ext4DirEntryFileType::Directory, dot_len)?,
                new_dir_entry(parent_ino, b"..", Ext4DirEntryFileType::Directory, dotdot_len)?,
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
        file_type: Ext4DirEntryFileType,
    ) -> SysResult<()> {
        if dir_inode.i_flags.contains(Ext4InodeFlags::INLINE_DATA) {
            return ret_errno("insert_dirent: inline_data directory unsupported", Errno::EOPNOTSUPP);
        }
        if !dir_inode.i_flags.contains(Ext4InodeFlags::EXTENTS) {
            return ret_errno("insert_dirent: non-extent directory unsupported", Errno::EOPNOTSUPP);
        }
        if name.is_empty() || name.len() > EXT4_MAX_NAME_LEN {
            return ret_errno("insert_dirent: invalid file name length", Errno::EINVAL);
        }
        if dir_inode.i_flags.contains(Ext4InodeFlags::INDEX) {
            return self.insert_indexed_dirent(dir_ino, dir_inode, name, child_ino, file_type);
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
        if dir_inode.i_flags.contains(Ext4InodeFlags::INLINE_DATA) {
            return ret_errno("remove_dirent: inline_data directory unsupported", Errno::EOPNOTSUPP);
        }
        if !dir_inode.i_flags.contains(Ext4InodeFlags::EXTENTS) {
            return ret_errno("remove_dirent: non-extent directory unsupported", Errno::EOPNOTSUPP);
        }
        if name.is_empty() || name.len() > EXT4_MAX_NAME_LEN {
            return ret_errno("remove_dirent: invalid file name length", Errno::EINVAL);
        }
        if dir_inode.i_flags.contains(Ext4InodeFlags::INDEX) {
            return self.remove_indexed_dirent(dir_ino, dir_inode, name);
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
        if dir_inode.i_flags.contains(Ext4InodeFlags::INLINE_DATA) {
            return ret_errno("patch_dotdot: inline_data directory unsupported", Errno::EOPNOTSUPP);
        }
        if !dir_inode.i_flags.contains(Ext4InodeFlags::EXTENTS) {
            return ret_errno("patch_dotdot: non-extent directory unsupported", Errno::EOPNOTSUPP);
        }
        if dir_inode.i_flags.contains(Ext4InodeFlags::INDEX) {
            return self.patch_indexed_dotdot(dir_ino, dir_inode, parent_ino);
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

    fn read_directory_logical_raw(&self, dir_ino: u32, dir_inode: &Ext4Inode, lblk: u32) -> SysResult<(u64, Vec<u8>)> {
        let root = self.parse_extent_root(dir_inode)?;
        let pblk = lookup_lblk_mut(self, root, dir_ino, dir_inode.i_generation, lblk)?
            .ok_or_else(|| debug_errno("read_directory_logical_raw: missing directory data block", Errno::EIO))?;
        Ok((pblk, self.read_fs_block(pblk)?))
    }

    fn insert_indexed_dirent(
        &self,
        dir_ino: u32,
        dir_inode: &mut Ext4Inode,
        name: &[u8],
        child_ino: u32,
        file_type: Ext4DirEntryFileType,
    ) -> SysResult<()> {
        let (root_pblk, root_raw) = self.read_directory_logical_raw(dir_ino, dir_inode, 0)?;
        let (hash_version, _, _) = self.parse_htree_root(&root_raw)?;
        let hash = htree_hash(name, self.hash_seed, hash_version)?;
        let leaf = self.find_indexed_directory_leaf(dir_ino, dir_inode, hash, root_pblk, root_raw)?;

        let mut dir_block = self.read_dir_block(dir_ino, dir_inode.i_generation, leaf.leaf_pblk)?;
        let old_block = dir_block.clone();
        if insert_dirent_into_block(&mut dir_block, name, child_ino, file_type)? {
            self.write_dir_block(dir_ino, dir_inode.i_generation, &mut dir_block)?;
            if let Err(err) = self.write_inode(dir_inode) {
                let mut restore = old_block;
                let _ = self.write_dir_block(dir_ino, dir_inode.i_generation, &mut restore);
                return Err(err);
            }
            return Ok(());
        }

        self.split_indexed_directory_leaf(dir_ino, dir_inode, leaf, hash_version, hash, name, child_ino, file_type)
    }

    fn remove_indexed_dirent(&self, dir_ino: u32, dir_inode: &Ext4Inode, name: &[u8]) -> SysResult<(u32, u8)> {
        let (_, root_raw) = self.read_directory_logical_raw(dir_ino, dir_inode, 0)?;
        let (hash_version, root, indirect_levels) = self.parse_htree_root(&root_raw)?;
        let hash = htree_hash(name, self.hash_seed, hash_version)?;
        let root_start = htree_find_position(&root_raw, root, hash);

        match indirect_levels {
            0 => {
                let mut pos = root_start;
                while pos < root.count as usize {
                    if pos != root_start && !htree_hash_matches(&root_raw, root, pos, hash) {
                        break;
                    }
                    let leaf_lblk = htree_entry_block(&root_raw, root, pos)?;
                    if let Some(removed) = self.try_remove_indexed_leaf(dir_ino, dir_inode, leaf_lblk, name)? {
                        return Ok(removed);
                    }
                    pos += 1;
                }
            }
            1 => {
                let mut root_pos = root_start;
                while root_pos < root.count as usize {
                    if root_pos != root_start && !htree_hash_matches(&root_raw, root, root_pos, hash) {
                        break;
                    }

                    let node_lblk = htree_entry_block(&root_raw, root, root_pos)?;
                    let (_, node_raw) = self.read_directory_logical_raw(dir_ino, dir_inode, node_lblk)?;
                    let node = self.parse_htree_node(&node_raw)?;
                    let node_start = if root_pos == root_start {
                        htree_find_position(&node_raw, node, hash)
                    } else {
                        0
                    };

                    let mut node_pos = node_start;
                    while node_pos < node.count as usize {
                        if node_pos != node_start && !htree_hash_matches(&node_raw, node, node_pos, hash) {
                            break;
                        }
                        let leaf_lblk = htree_entry_block(&node_raw, node, node_pos)?;
                        if let Some(removed) = self.try_remove_indexed_leaf(dir_ino, dir_inode, leaf_lblk, name)? {
                            return Ok(removed);
                        }
                        node_pos += 1;
                    }

                    root_pos += 1;
                }
            }
            _ => return Err(Errno::EOPNOTSUPP),
        }

        ret_errno("remove_indexed_dirent: name not found", Errno::ENOENT)
    }

    fn try_remove_indexed_leaf(
        &self,
        dir_ino: u32,
        dir_inode: &Ext4Inode,
        leaf_lblk: u32,
        name: &[u8],
    ) -> SysResult<Option<(u32, u8)>> {
        let (leaf_pblk, _) = self.read_directory_logical_raw(dir_ino, dir_inode, leaf_lblk)?;
        let mut dir_block = self.read_dir_block(dir_ino, dir_inode.i_generation, leaf_pblk)?;
        let Some(removed) = remove_dirent_from_block(&mut dir_block, name)? else {
            return Ok(None);
        };
        self.write_dir_block(dir_ino, dir_inode.i_generation, &mut dir_block)?;
        Ok(Some(removed))
    }

    fn patch_indexed_dotdot(&self, dir_ino: u32, dir_inode: &Ext4Inode, parent_ino: u32) -> SysResult<()> {
        let (pblk, mut raw) = self.read_directory_logical_raw(dir_ino, dir_inode, 0)?;
        let dot_len = get_u16_le(&raw, 4)? as usize;
        let header_end = dot_len
            .checked_add(DIR_ENTRY_HEADER_SIZE)
            .ok_or_else(|| debug_errno("patch_indexed_dotdot: dotdot header overflow", Errno::EINVAL))?;
        if dot_len == 0 || header_end > raw.len() {
            return ret_errno("patch_indexed_dotdot: invalid dot entry length", Errno::EIO);
        }
        let name_len = get_u8(&raw, dot_len + 6)? as usize;
        let name_off = dot_len
            .checked_add(DIR_ENTRY_HEADER_SIZE)
            .ok_or_else(|| debug_errno("patch_indexed_dotdot: name offset overflow", Errno::EINVAL))?;
        let name_end = name_off
            .checked_add(name_len)
            .ok_or_else(|| debug_errno("patch_indexed_dotdot: name end overflow", Errno::EINVAL))?;
        if name_end > raw.len() || &raw[name_off..name_end] != b".." {
            return ret_errno("patch_indexed_dotdot: missing '..' entry", Errno::EIO);
        }

        put_u32_le(&mut raw, dot_len, parent_ino)?;
        self.set_dirent_tail_checksum(dir_ino, dir_inode.i_generation, &mut raw)?;
        self.write_fs_block(pblk, &raw)
    }

    fn find_indexed_directory_leaf(
        &self,
        dir_ino: u32,
        dir_inode: &Ext4Inode,
        hash: u32,
        root_pblk: u64,
        root_raw: Vec<u8>,
    ) -> SysResult<HTreeLeafPath> {
        let (_, root, indirect_levels) = self.parse_htree_root(&root_raw)?;
        let root_position = htree_find_position(&root_raw, root, hash);

        match indirect_levels {
            0 => {
                let leaf_lblk = htree_entry_block(&root_raw, root, root_position)?;
                let (leaf_pblk, _) = self.read_directory_logical_raw(dir_ino, dir_inode, leaf_lblk)?;
                Ok(HTreeLeafPath {
                    index_pblk: root_pblk,
                    index_raw: root_raw,
                    index_node: root,
                    index_position: root_position,
                    leaf_pblk,
                })
            }
            1 => {
                let node_lblk = htree_entry_block(&root_raw, root, root_position)?;
                let (node_pblk, node_raw) = self.read_directory_logical_raw(dir_ino, dir_inode, node_lblk)?;
                let node = self.parse_htree_node(&node_raw)?;
                let node_position = htree_find_position(&node_raw, node, hash);
                let leaf_lblk = htree_entry_block(&node_raw, node, node_position)?;
                let (leaf_pblk, _) = self.read_directory_logical_raw(dir_ino, dir_inode, leaf_lblk)?;
                Ok(HTreeLeafPath {
                    index_pblk: node_pblk,
                    index_raw: node_raw,
                    index_node: node,
                    index_position: node_position,
                    leaf_pblk,
                })
            }
            _ => Err(Errno::EOPNOTSUPP),
        }
    }

    fn parse_htree_root(&self, raw: &[u8]) -> SysResult<(HTreeHashVersion, HTreeNode, u8)> {
        let block_size = self.block_size as usize;
        if raw.len() != block_size
            || get_u16_le(raw, 4)? != DX_ROOT_INFO_OFF as u16 - 12
            || get_u16_le(raw, 16)? as usize != block_size - 12
            || get_u32_le(raw, DX_ROOT_INFO_OFF)? != 0
            || get_u8(raw, DX_ROOT_INFO_OFF + 5)? != 8
            || get_u8(raw, DX_ROOT_INFO_OFF + 7)? != 0
        {
            return Err(Errno::EIO);
        }

        let hash_version = HTreeHashVersion::from_raw(get_u8(raw, DX_ROOT_INFO_OFF + 4)?, self.flags)?;
        let indirect_levels = get_u8(raw, DX_ROOT_INFO_OFF + 6)?;
        if indirect_levels > 1 {
            return Err(Errno::EOPNOTSUPP);
        }

        let entry_space = block_size
            .checked_sub(DX_ROOT_ENTRY_OFF)
            .and_then(|space| space.checked_sub(self.htree_tail_size()))
            .ok_or_else(|| debug_errno("parse_htree_root: entry space underflow", Errno::EIO))?;
        let expected_limit = entry_space / DX_ENTRY_SIZE;
        let node = self.parse_htree_entries(raw, DX_ROOT_ENTRY_OFF, expected_limit)?;
        Ok((hash_version, node, indirect_levels))
    }

    fn parse_htree_node(&self, raw: &[u8]) -> SysResult<HTreeNode> {
        let block_size = self.block_size as usize;
        if raw.len() != block_size || get_u16_le(raw, 4)? as usize != block_size {
            return Err(Errno::EIO);
        }
        let entry_space = block_size
            .checked_sub(DX_NODE_ENTRY_OFF)
            .and_then(|space| space.checked_sub(self.htree_tail_size()))
            .ok_or_else(|| debug_errno("parse_htree_node: entry space underflow", Errno::EIO))?;
        let expected_limit = entry_space / DX_ENTRY_SIZE;
        self.parse_htree_entries(raw, DX_NODE_ENTRY_OFF, expected_limit)
    }

    fn parse_htree_entries(&self, raw: &[u8], entry_offset: usize, expected_limit: usize) -> SysResult<HTreeNode> {
        if entry_offset + 4 > raw.len() || expected_limit > u16::MAX as usize {
            return Err(Errno::EIO);
        }
        let limit = get_u16_le(raw, entry_offset)?;
        let count = get_u16_le(raw, entry_offset + 2)?;
        if limit as usize != expected_limit || count == 0 || count > limit {
            return Err(Errno::EIO);
        }
        let entries_end = entry_offset
            .checked_add(
                (limit as usize)
                    .checked_mul(DX_ENTRY_SIZE)
                    .ok_or_else(|| debug_errno("parse_htree_entries: entries span overflow", Errno::EINVAL))?,
            )
            .ok_or_else(|| debug_errno("parse_htree_entries: entries end overflow", Errno::EINVAL))?;
        if entries_end > raw.len() {
            return Err(Errno::EIO);
        }
        Ok(HTreeNode {
            entry_offset,
            limit,
            count,
        })
    }

    fn htree_tail_size(&self) -> usize {
        if self.metadata_csum { 8 } else { 0 }
    }

    fn split_indexed_directory_leaf(
        &self,
        dir_ino: u32,
        dir_inode: &mut Ext4Inode,
        mut leaf: HTreeLeafPath,
        hash_version: HTreeHashVersion,
        hash: u32,
        name: &[u8],
        child_ino: u32,
        file_type: Ext4DirEntryFileType,
    ) -> SysResult<()> {
        if leaf.index_node.count >= leaf.index_node.limit {
            return Err(Errno::EOPNOTSUPP);
        }

        let original_inode = dir_inode.clone();
        let old_index_raw = leaf.index_raw.clone();
        let old_leaf_block = self.read_dir_block(dir_ino, dir_inode.i_generation, leaf.leaf_pblk)?;
        let mut entries = self.collect_hashed_directory_entries(&old_leaf_block, hash_version)?;
        entries.push(HashedDirectoryEntry {
            hash,
            ino: child_ino,
            name: name.to_vec(),
            file_type: file_type.as_u8(),
            rec_len: dirent_min_len(name.len())?,
        });
        entries.sort_by_key(|entry| entry.hash);

        let usable_len = writable_dir_bytes(self.block_size as usize, self.metadata_csum)?;
        let mid = split_hashed_entries(&entries, usable_len)?;
        let new_hash = entries[mid]
            .hash
            .wrapping_add(u32::from(entries[mid].hash == entries[mid - 1].hash));
        let new_lblk = u32::try_from(dir_inode.i_size / self.block_size as u64)
            .map_err(|_| debug_errno("split_indexed_directory_leaf: new logical block overflow", Errno::EFBIG))?;
        let new_pblk = self.alloc_block()?;

        let result = (|| -> SysResult<()> {
            self.insert_extent_1blk(dir_ino, dir_inode, new_lblk, new_pblk)?;
            dir_inode.i_size = dir_inode
                .i_size
                .checked_add(self.block_size as u64)
                .ok_or_else(|| debug_errno("split_indexed_directory_leaf: directory size overflow", Errno::EINVAL))?;

            let mut left = dir_block_from_hashed_entries(leaf.leaf_pblk, &entries[..mid], usable_len)?;
            let mut right = dir_block_from_hashed_entries(new_pblk, &entries[mid..], usable_len)?;

            leaf.index_node = htree_insert_entry(
                &mut leaf.index_raw,
                leaf.index_node,
                leaf.index_position,
                new_hash,
                new_lblk,
            )?;
            self.set_htree_block_checksum(dir_inode, &mut leaf.index_raw, leaf.index_node)?;

            self.write_dir_block(dir_ino, dir_inode.i_generation, &mut right)?;
            self.write_dir_block(dir_ino, dir_inode.i_generation, &mut left)?;
            self.write_fs_block(leaf.index_pblk, &leaf.index_raw)?;
            self.write_inode(dir_inode)
        })();

        if let Err(err) = result {
            let mut restore_leaf = old_leaf_block;
            let _ = self.write_dir_block(dir_ino, original_inode.i_generation, &mut restore_leaf);
            let _ = self.write_fs_block(leaf.index_pblk, &old_index_raw);
            let mut restore_inode = original_inode.clone();
            let _ = self.write_inode(&mut restore_inode);
            let _ = self.free_block(new_pblk);
            *dir_inode = original_inode;
            return Err(err);
        }

        Ok(())
    }

    fn collect_hashed_directory_entries(
        &self,
        block: &DirBlock,
        hash_version: HTreeHashVersion,
    ) -> SysResult<Vec<HashedDirectoryEntry>> {
        let mut entries = Vec::new();
        for entry in &block.entries {
            if entry.inode == 0 || entry.name_len == 0 {
                continue;
            }
            let name = entry.name_slice().to_vec();
            entries.push(HashedDirectoryEntry {
                hash: htree_hash(&name, self.hash_seed, hash_version)?,
                ino: entry.inode,
                name,
                file_type: entry.file_type,
                rec_len: dirent_min_len(entry.name_len as usize)?,
            });
        }
        Ok(entries)
    }

    fn set_htree_block_checksum(&self, inode: &Ext4Inode, raw: &mut [u8], node: HTreeNode) -> SysResult<()> {
        if !self.metadata_csum {
            return Ok(());
        }

        let tail = node
            .entry_offset
            .checked_add(node.limit as usize * DX_ENTRY_SIZE)
            .ok_or_else(|| debug_errno("set_htree_block_checksum: tail offset overflow", Errno::EINVAL))?;
        if tail
            .checked_add(8)
            .ok_or_else(|| debug_errno("set_htree_block_checksum: tail end overflow", Errno::EINVAL))?
            > raw.len()
        {
            return Err(Errno::EIO);
        }

        put_u32_le(raw, tail + 4, 0)?;
        let checksum_len = node
            .entry_offset
            .checked_add(node.count as usize * DX_ENTRY_SIZE)
            .ok_or_else(|| debug_errno("set_htree_block_checksum: checksum length overflow", Errno::EINVAL))?;
        let mut checksum = self.checksum_seed;
        checksum = crc32c_update(checksum, &inode.ino.to_le_bytes());
        checksum = crc32c_update(checksum, &inode.i_generation.to_le_bytes());
        checksum = crc32c_update(checksum, &raw[..checksum_len]);
        checksum = crc32c_update(checksum, &raw[tail..tail + 8]);
        put_u32_le(raw, tail + 4, checksum)
    }

    fn set_dirent_tail_checksum(&self, dir_ino: u32, dir_generation: u32, raw: &mut [u8]) -> SysResult<()> {
        if !self.metadata_csum {
            return Ok(());
        }
        let Some(off) = dir_entry_tail_offset(raw)? else {
            return Ok(());
        };

        put_u32_le(raw, off + 8, 0)?;
        let mut checksum = self.checksum_seed;
        checksum = crc32c_update(checksum, &dir_ino.to_le_bytes());
        checksum = crc32c_update(checksum, &dir_generation.to_le_bytes());
        checksum = crc32c_update(checksum, &raw[..off]);
        put_u32_le(raw, off + 8, checksum)
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

fn dir_entry_tail_offset(raw: &[u8]) -> SysResult<Option<usize>> {
    if raw.len() < DIR_ENTRY_TAIL_SIZE {
        return Ok(None);
    }

    let off = raw.len() - DIR_ENTRY_TAIL_SIZE;
    let reserved_zero1 = get_u32_le(raw, off)?;
    let rec_len = get_u16_le(raw, off + 4)?;
    let reserved_zero2 = get_u8(raw, off + 6)?;
    let reserved_ft = get_u8(raw, off + 7)?;

    if reserved_zero1 == 0
        && rec_len == DIR_ENTRY_TAIL_SIZE as u16
        && reserved_zero2 == 0
        && reserved_ft == EXT4_DIRENTRY_DIR_CSUM
    {
        Ok(Some(off))
    } else {
        Ok(None)
    }
}

fn new_dir_entry(inode: u32, name: &[u8], file_type: Ext4DirEntryFileType, rec_len: u16) -> SysResult<DirEntry2> {
    if name.len() > EXT4_MAX_NAME_LEN {
        return ret_errno("new_dir_entry: file name is too long", Errno::EINVAL);
    }

    let mut name_buf = [0u8; EXT4_MAX_NAME_LEN];
    name_buf[..name.len()].copy_from_slice(name);
    Ok(DirEntry2 {
        inode,
        rec_len,
        name_len: name.len() as u8,
        file_type: file_type.as_u8(),
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

fn dir_block_from_hashed_entries(
    pblk: u64,
    entries: &[HashedDirectoryEntry],
    usable_len: usize,
) -> SysResult<DirBlock> {
    if entries.is_empty() {
        return ret_errno("dir_block_from_hashed_entries: empty leaf", Errno::EIO);
    }

    let mut dir_entries = Vec::new();
    let mut offset = 0usize;
    for (index, entry) in entries.iter().enumerate() {
        let rec_len = if index + 1 == entries.len() {
            usable_len
                .checked_sub(offset)
                .ok_or_else(|| debug_errno("dir_block_from_hashed_entries: rec_len underflow", Errno::EINVAL))?
        } else {
            entry.rec_len as usize
        };
        if rec_len < entry.rec_len as usize || rec_len > u16::MAX as usize {
            return ret_errno("dir_block_from_hashed_entries: invalid rec_len", Errno::EIO);
        }

        let mut name = [0u8; EXT4_MAX_NAME_LEN];
        if entry.name.len() > EXT4_MAX_NAME_LEN {
            return ret_errno("dir_block_from_hashed_entries: file name is too long", Errno::EINVAL);
        }
        name[..entry.name.len()].copy_from_slice(&entry.name);
        dir_entries.push(DirEntry2 {
            inode: entry.ino,
            rec_len: rec_len as u16,
            name_len: entry.name.len() as u8,
            file_type: entry.file_type,
            name,
            entry_off: 0,
        });
        offset = offset
            .checked_add(entry.rec_len as usize)
            .ok_or_else(|| debug_errno("dir_block_from_hashed_entries: offset overflow", Errno::EINVAL))?;
    }

    Ok(DirBlock {
        pblk,
        raw: Vec::new(),
        entries: dir_entries,
    })
}

fn insert_dirent_into_block(
    block: &mut DirBlock,
    name: &[u8],
    child_ino: u32,
    file_type: Ext4DirEntryFileType,
) -> SysResult<bool> {
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

fn htree_entry_offset(node: HTreeNode, index: usize) -> SysResult<usize> {
    if index >= node.count as usize {
        return Err(Errno::EIO);
    }
    node.entry_offset
        .checked_add(
            index
                .checked_mul(DX_ENTRY_SIZE)
                .ok_or_else(|| debug_errno("htree_entry_offset: index overflow", Errno::EINVAL))?,
        )
        .ok_or_else(|| debug_errno("htree_entry_offset: offset overflow", Errno::EINVAL))
}

fn htree_entry_hash(raw: &[u8], node: HTreeNode, index: usize) -> SysResult<u32> {
    get_u32_le(raw, htree_entry_offset(node, index)?)
}

fn htree_entry_block(raw: &[u8], node: HTreeNode, index: usize) -> SysResult<u32> {
    get_u32_le(raw, htree_entry_offset(node, index)? + 4)
}

fn htree_insert_entry(
    raw: &mut [u8],
    mut node: HTreeNode,
    position: usize,
    hash: u32,
    logical_block: u32,
) -> SysResult<HTreeNode> {
    if position >= node.count as usize || node.count >= node.limit {
        return Err(Errno::EIO);
    }

    let insert_index = position
        .checked_add(1)
        .ok_or_else(|| debug_errno("htree_insert_entry: insert index overflow", Errno::EINVAL))?;
    let insert_offset = node
        .entry_offset
        .checked_add(
            insert_index
                .checked_mul(DX_ENTRY_SIZE)
                .ok_or_else(|| debug_errno("htree_insert_entry: insert offset span overflow", Errno::EINVAL))?,
        )
        .ok_or_else(|| debug_errno("htree_insert_entry: insert offset overflow", Errno::EINVAL))?;
    let old_end = node
        .entry_offset
        .checked_add(node.count as usize * DX_ENTRY_SIZE)
        .ok_or_else(|| debug_errno("htree_insert_entry: old end overflow", Errno::EINVAL))?;
    let new_end = old_end
        .checked_add(DX_ENTRY_SIZE)
        .ok_or_else(|| debug_errno("htree_insert_entry: new end overflow", Errno::EINVAL))?;
    if new_end > raw.len() {
        return Err(Errno::EIO);
    }

    raw.copy_within(insert_offset..old_end, insert_offset + DX_ENTRY_SIZE);
    put_u32_le(raw, insert_offset, hash)?;
    put_u32_le(raw, insert_offset + 4, logical_block)?;
    node.count += 1;
    put_u16_le(raw, node.entry_offset + 2, node.count)?;
    Ok(node)
}

fn htree_hash_matches(raw: &[u8], node: HTreeNode, index: usize, hash: u32) -> bool {
    htree_entry_hash(raw, node, index).is_ok_and(|entry_hash| entry_hash & !1 == hash)
}

fn htree_find_position(raw: &[u8], node: HTreeNode, hash: u32) -> usize {
    let mut low = 1usize;
    let mut high = node.count as usize;
    while low < high {
        let middle = low + (high - low) / 2;
        if htree_entry_hash(raw, node, middle).unwrap_or(u32::MAX) > hash {
            high = middle;
        } else {
            low = middle + 1;
        }
    }
    low - 1
}

fn split_hashed_entries(entries: &[HashedDirectoryEntry], usable_len: usize) -> SysResult<usize> {
    if entries.len() < 2 {
        return Err(Errno::EIO);
    }

    let mut current_size = 0usize;
    for (index, entry) in entries.iter().enumerate() {
        let rec_len = entry.rec_len as usize;
        if index > 0
            && current_size
                .checked_add(rec_len)
                .ok_or_else(|| debug_errno("split_hashed_entries: size overflow", Errno::EINVAL))?
                > usable_len / 2
        {
            return Ok(index);
        }
        current_size = current_size
            .checked_add(rec_len)
            .ok_or_else(|| debug_errno("split_hashed_entries: current size overflow", Errno::EINVAL))?;
    }

    Ok(entries.len() - 1)
}

fn htree_hash(name: &[u8], seed: [u32; 4], version: HTreeHashVersion) -> SysResult<u32> {
    if name.is_empty() || name.len() > EXT4_MAX_NAME_LEN {
        return Err(Errno::EOPNOTSUPP);
    }

    let mut state = seed;
    match version {
        HTreeHashVersion::Tea | HTreeHashVersion::TeaUnsigned => {
            let mut remaining = name;
            while !remaining.is_empty() {
                let data = htree_prep_hashbuf(remaining, 16, version.unsigned());
                htree_tea(&mut state, data);
                remaining = remaining.get(cmp::min(16, remaining.len())..).ok_or(Errno::EIO)?;
            }
            Ok(htree_normalize_hash(state[0]))
        }
        HTreeHashVersion::Legacy | HTreeHashVersion::LegacyUnsigned => {
            Ok(htree_normalize_hash(htree_legacy_hash(name, version.unsigned())))
        }
        HTreeHashVersion::HalfMd4 | HTreeHashVersion::HalfMd4Unsigned => {
            let mut remaining = name;
            while !remaining.is_empty() {
                let data = htree_prep_hashbuf(remaining, 32, version.unsigned());
                htree_half_md4(&mut state, data);
                remaining = remaining.get(cmp::min(32, remaining.len())..).ok_or(Errno::EIO)?;
            }
            Ok(htree_normalize_hash(state[1]))
        }
    }
}

fn htree_normalize_hash(mut hash: u32) -> u32 {
    hash &= !1;
    if hash == 0xffff_fffe {
        hash = 0xffff_fffc;
    }
    hash
}

fn htree_legacy_hash(name: &[u8], unsigned_char: bool) -> u32 {
    let mut h1 = 0x12a3_fe2d_u32;
    let mut h2 = 0x37ab_e8f9_u32;
    let multi = 0x6d22_f5_u32;

    for &byte in name {
        let value = htree_char_value(byte, unsigned_char);
        let mut h0 = h2.wrapping_add(h1 ^ value.wrapping_mul(multi));
        if h0 & 0x8000_0000 != 0 {
            h0 = h0.wrapping_sub(0x7fff_ffff);
        }
        h2 = h1;
        h1 = h0;
    }

    h1.wrapping_shl(1)
}

fn htree_prep_hashbuf(name: &[u8], output_len: usize, unsigned_char: bool) -> [u32; 8] {
    let padding =
        (name.len() as u32) | ((name.len() as u32) << 8) | ((name.len() as u32) << 16) | ((name.len() as u32) << 24);
    let output_words = output_len / core::mem::size_of::<u32>();
    let len = cmp::min(name.len(), output_len);
    let mut data = [0u32; 8];
    let mut word = padding;
    let mut word_index = 0usize;

    for (i, &byte) in name[..len].iter().enumerate() {
        if i % 4 == 0 {
            word = padding;
        }
        word = word.wrapping_shl(8).wrapping_add(htree_char_value(byte, unsigned_char));
        if i % 4 == 3 {
            data[word_index] = word;
            word_index += 1;
            word = padding;
        }
    }

    if word_index < output_words {
        data[word_index] = word;
        word_index += 1;
    }
    while word_index < output_words {
        data[word_index] = padding;
        word_index += 1;
    }

    data
}

fn htree_char_value(byte: u8, unsigned_char: bool) -> u32 {
    if unsigned_char {
        byte as u32
    } else {
        (byte as i8 as i32) as u32
    }
}

fn htree_tea(state: &mut [u32; 4], data: [u32; 8]) {
    let mut x = state[0];
    let mut y = state[1];
    for i in 1..=16u32 {
        let sum = i.wrapping_mul(0x9e37_79b9);
        x = x.wrapping_add(
            y.wrapping_shl(4).wrapping_add(data[0]) ^ y.wrapping_add(sum) ^ y.wrapping_shr(5).wrapping_add(data[1]),
        );
        y = y.wrapping_add(
            x.wrapping_shl(4).wrapping_add(data[2]) ^ x.wrapping_add(sum) ^ x.wrapping_shr(5).wrapping_add(data[3]),
        );
    }
    state[0] = state[0].wrapping_add(x);
    state[1] = state[1].wrapping_add(y);
}

fn htree_half_md4(state: &mut [u32; 4], data: [u32; 8]) {
    let mut a = state[0];
    let mut b = state[1];
    let mut c = state[2];
    let mut d = state[3];

    md4_ff(&mut a, b, c, d, data[0], 3);
    md4_ff(&mut d, a, b, c, data[1], 7);
    md4_ff(&mut c, d, a, b, data[2], 11);
    md4_ff(&mut b, c, d, a, data[3], 19);
    md4_ff(&mut a, b, c, d, data[4], 3);
    md4_ff(&mut d, a, b, c, data[5], 7);
    md4_ff(&mut c, d, a, b, data[6], 11);
    md4_ff(&mut b, c, d, a, data[7], 19);

    md4_gg(&mut a, b, c, d, data[1], 3);
    md4_gg(&mut d, a, b, c, data[3], 5);
    md4_gg(&mut c, d, a, b, data[5], 9);
    md4_gg(&mut b, c, d, a, data[7], 13);
    md4_gg(&mut a, b, c, d, data[0], 3);
    md4_gg(&mut d, a, b, c, data[2], 5);
    md4_gg(&mut c, d, a, b, data[4], 9);
    md4_gg(&mut b, c, d, a, data[6], 13);

    md4_hh(&mut a, b, c, d, data[3], 3);
    md4_hh(&mut d, a, b, c, data[7], 9);
    md4_hh(&mut c, d, a, b, data[2], 11);
    md4_hh(&mut b, c, d, a, data[6], 15);
    md4_hh(&mut a, b, c, d, data[1], 3);
    md4_hh(&mut d, a, b, c, data[5], 9);
    md4_hh(&mut c, d, a, b, data[0], 11);
    md4_hh(&mut b, c, d, a, data[4], 15);

    state[0] = state[0].wrapping_add(a);
    state[1] = state[1].wrapping_add(b);
    state[2] = state[2].wrapping_add(c);
    state[3] = state[3].wrapping_add(d);
}

fn md4_ff(target: &mut u32, b: u32, c: u32, d: u32, x: u32, shift: u32) {
    *target = target
        .wrapping_add((b & c) | (!b & d))
        .wrapping_add(x)
        .rotate_left(shift);
}

fn md4_gg(target: &mut u32, b: u32, c: u32, d: u32, x: u32, shift: u32) {
    *target = target
        .wrapping_add((b & c) | (b & d) | (c & d))
        .wrapping_add(x)
        .wrapping_add(0x5a82_7999)
        .rotate_left(shift);
}

fn md4_hh(target: &mut u32, b: u32, c: u32, d: u32, x: u32, shift: u32) {
    *target = target
        .wrapping_add(b ^ c ^ d)
        .wrapping_add(x)
        .wrapping_add(0x6ed9_eba1)
        .rotate_left(shift);
}
