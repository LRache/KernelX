use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::sync::Arc;
use alloc::vec::Vec;
use core::cmp;
use core::time::Duration;

use bitflags::bitflags;

use crate::arch;
use crate::driver::BlockDriverOps;
use crate::driver::chosen::kclock;
use crate::fs::file::DirResult;
use crate::fs::filesystem::SuperBlockOps;
use crate::fs::inode::Mode;
use crate::fs::{FileType, InodeOps, Owner};
use crate::kernel::config;
use crate::kernel::errno::{Errno, SysResult};
use crate::kernel::mm::PhysPageFrame;
use crate::kernel::uapi::{FileStat, Statfs, StatfsFlags};
use crate::klib::SleepLock;
use crate::klib::lru::LRUCache;

use super::inode::Inode;
use super::utils::{EXT4_CRC32_INIT, crc32c};

const SUPERBLOCK_OFFSET: u64 = 1024;
const SUPERBLOCK_SIZE: usize = 1024;
const EXT4_SUPER_MAGIC: u16 = 0xef53;
const EXT4_SUPER_MAGIC_STATFS: u64 = 0xef53;
const ROOT_INO: u32 = 2;
const EXT4_NAME_LEN: usize = 255;
const EXT4_MIN_DESC_SIZE: u16 = 32;
const EXT4_MIN_INODE_SIZE: u16 = 128;
const EXT4_MAX_BLOCK_SIZE: u64 = 4096;
const EXT4_EXTENT_MAGIC: u16 = 0xf30a;
const EXT4_EXTENT_HEADER_SIZE: usize = 12;
const EXT4_EXTENT_ENTRY_SIZE: usize = 12;
const EXT4_DIR_ENTRY_HEADER_SIZE: usize = 8;
const EXT4_DX_ROOT_INFO_OFFSET: usize = 24;
const EXT4_DX_ROOT_ENTRY_OFFSET: usize = 32;
const EXT4_DX_NODE_ENTRY_OFFSET: usize = 8;
const EXT4_DX_ENTRY_SIZE: usize = 8;
const EXT4_EXTENT_UNWRITTEN: u16 = 0x8000;
const EXT4_EXTENT_MAX_INITIALIZED_LEN: u16 = 0x7fff;
const INODE_METADATA_CACHE_SIZE: usize = 16;
const GROUP_DESCRIPTOR_CACHE_SIZE: usize = 16;
const EXT4_DIRENT_TAIL_FILE_TYPE: u8 = 0xde;
const EXT4_DIRENT_TAIL_SIZE: usize = 12;

const EXT4_CHECKSUM_CRC32C: u8 = 1;
const EXT4_FEATURE_RO_COMPAT_GDT_CSUM: u32 = 0x0010;
const EXT4_FEATURE_RO_COMPAT_BIGALLOC: u32 = 0x0200;
const EXT4_FEATURE_RO_COMPAT_METADATA_CSUM: u32 = 0x0400;
const EXT4_INODE_BLOCK_SIZE: u64 = 512;
const EXT4_SUPERBLOCK_FLAGS_UNSIGNED_HASH: u32 = 0x0002;

const EXT4_INODE_FLAG_IMMUTABLE: u32 = 0x0000_0010;
const EXT4_INODE_FLAG_APPEND: u32 = 0x0000_0020;
const EXT4_INODE_FLAG_INDEX: u32 = 0x0000_1000;
const EXT4_INODE_FLAG_EXTENTS: u32 = 0x0008_0000;
const EXT4_INLINE_DATA_FL: u32 = 0x1000_0000;

const EXT4_DE_UNKNOWN: u8 = 0;
const EXT4_DE_REG_FILE: u8 = 1;
const EXT4_DE_DIR: u8 = 2;
const EXT4_DE_CHRDEV: u8 = 3;
const EXT4_DE_BLKDEV: u8 = 4;
const EXT4_DE_FIFO: u8 = 5;
const EXT4_DE_SOCK: u8 = 6;
const EXT4_DE_SYMLINK: u8 = 7;

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

bitflags! {
    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    struct Ext4IncompatFeatures: u32 {
        const COMPRESSION = 0x0001;
        const FILETYPE = 0x0002;
        const RECOVER = 0x0004;
        const JOURNAL_DEV = 0x0008;
        const META_BG = 0x0010;
        const EXTENTS = 0x0040;
        const BIT64 = 0x0080;
        const FLEX_BG = 0x0200;
        const CSUM_SEED = 0x2000;
        const INLINE_DATA = 0x8000;
        const ENCRYPT = 0x0001_0000;
        const CASEFOLD = 0x0002_0000;
    }
}

impl Ext4IncompatFeatures {
    fn supported() -> Self {
        Self::FILETYPE | Self::EXTENTS | Self::BIT64 | Self::FLEX_BG | Self::CSUM_SEED
    }
}

#[derive(Clone, Copy)]
struct Ext4Info {
    block_size: u64,
    blocks_count: u64,
    free_blocks_count: u64,
    inodes_count: u32,
    free_inodes_count: u32,
    first_data_block: u32,
    blocks_per_group: u32,
    inodes_per_group: u32,
    inode_size: u16,
    desc_size: u16,
    features_readonly: u32,
    features_incompatible: Ext4IncompatFeatures,
    hash_seed: [u32; 4],
    flags: u32,
    checksum_seed: u32,
    checksum_type: u8,
}

#[derive(Clone)]
struct GroupDescriptor {
    block_bitmap_block: u64,
    inode_bitmap_block: u64,
    inode_table_block: u64,
    free_blocks_count: u64,
    free_inodes_count: u64,
    used_dirs_count: u64,
    raw: Vec<u8>,
}

#[derive(Clone)]
struct Ext4InodeRaw {
    ino: u32,
    mode: u16,
    uid: u32,
    gid: u32,
    size: u64,
    atime: u32,
    ctime: u32,
    mtime: u32,
    generation: u32,
    links_count: u16,
    blocks_count: u64,
    flags: u32,
    block: [u8; 60],
    raw: Vec<u8>,
}

#[derive(Clone)]
struct CachedInodeRaw {
    inode: Ext4InodeRaw,
    dirty: bool,
}

#[derive(Clone, Copy)]
struct ExtentHeader {
    entries: u16,
    max_entries: u16,
    depth: u16,
}

#[derive(Clone, Copy)]
struct ExtentIndex {
    first_block: u32,
    leaf: u64,
}

#[derive(Clone, Copy)]
struct Extent {
    first_block: u32,
    len: u16,
    pblock: u64,
    unwritten: bool,
}

#[derive(Clone, Copy)]
enum BlockMapping {
    Data(u64),
    Hole,
    Unwritten,
}

struct DirectoryEntry {
    ino: u32,
    name: String,
    file_type: FileType,
}

struct DirectoryEntryLocation {
    block: Vec<u8>,
    block_offset: usize,
    previous_entry_offset: Option<usize>,
    entry_offset: usize,
    entry_len: usize,
    ino: u32,
    file_type: FileType,
}

#[derive(Clone, Copy)]
struct HTreeNode {
    entry_offset: usize,
    limit: u16,
    count: u16,
}

struct HTreeLeafPath {
    index_block: Vec<u8>,
    index_logical_block: u32,
    index_node: HTreeNode,
    index_position: usize,
    leaf_logical_block: u32,
}

struct HashedDirectoryEntry {
    hash: u32,
    ino: u32,
    name: Vec<u8>,
    file_type: u8,
    rec_len: usize,
}

struct AppendLeaf {
    node: Vec<u8>,
    header: ExtentHeader,
    block: Option<u64>,
}

#[derive(Clone, Copy)]
struct AllocatedBlockRun {
    first_block: u64,
    len: u16,
}

struct CachedPage {
    frame: Arc<PhysPageFrame>,
    dirty: bool,
}

pub(super) struct InodePageCache {
    pages: BTreeMap<usize, CachedPage>,
}

impl InodePageCache {
    const CAPACITY: usize = config::EXT4_INODE_PAGE_CACHE_SIZE;

    pub fn new() -> Self {
        Self { pages: BTreeMap::new() }
    }

    pub fn get_frame(&self, page_index: usize) -> Option<Arc<PhysPageFrame>> {
        self.pages.get(&page_index).map(|page| page.frame.clone())
    }

    pub fn insert_frame(&mut self, page_index: usize, frame: Arc<PhysPageFrame>) -> Arc<PhysPageFrame> {
        if Self::CAPACITY == 0 {
            return frame;
        }
        if let Some(page) = self.pages.get(&page_index) {
            return page.frame.clone();
        }

        self.shrink_for_insert();
        self.pages.insert(
            page_index,
            CachedPage {
                frame: frame.clone(),
                dirty: false,
            },
        );
        frame
    }

    pub fn copy_to_slice(&self, buf: &mut [u8], offset: usize, len: usize) {
        let mut copied = 0;
        while copied < len {
            let current = offset + copied;
            let page_index = current / arch::PGSIZE;
            let page_offset = current % arch::PGSIZE;
            let copy_len = cmp::min(len - copied, arch::PGSIZE - page_offset);

            if let Some(page) = self.pages.get(&page_index) {
                page.frame
                    .copy_to_slice(page_offset, &mut buf[copied..copied + copy_len]);
            }

            copied += copy_len;
        }
    }

    pub fn write_if_cached(&mut self, buf: &[u8], offset: usize) -> bool {
        let mut checked = 0;
        while checked < buf.len() {
            let current = offset + checked;
            let page_index = current / arch::PGSIZE;
            let page_offset = current % arch::PGSIZE;
            let copy_len = cmp::min(buf.len() - checked, arch::PGSIZE - page_offset);

            if !self.pages.contains_key(&page_index) {
                return false;
            }

            checked += copy_len;
        }

        let mut copied = 0;
        while copied < buf.len() {
            let current = offset + copied;
            let page_index = current / arch::PGSIZE;
            let page_offset = current % arch::PGSIZE;
            let copy_len = cmp::min(buf.len() - copied, arch::PGSIZE - page_offset);
            let page = self.pages.get_mut(&page_index).unwrap();

            page.frame.copy_from_slice(page_offset, &buf[copied..copied + copy_len]);
            page.dirty = true;

            copied += copy_len;
        }

        true
    }

    pub fn mark_dirty(&mut self, page_index: usize) {
        if let Some(page) = self.pages.get_mut(&page_index) {
            page.dirty = true;
        }
    }

    pub fn mark_clean(&mut self, page_index: usize) {
        if let Some(page) = self.pages.get_mut(&page_index) {
            page.dirty = false;
        }
    }

    pub fn dirty_pages(&self) -> Vec<usize> {
        self.pages
            .iter()
            .filter_map(|(page_index, page)| page.dirty.then_some(*page_index))
            .collect()
    }

    pub fn has_dirty_page(&self) -> bool {
        self.pages.values().any(|page| page.dirty)
    }

    pub fn discard_after_truncate(&mut self, new_size: usize) {
        let new_page_count = new_size.div_ceil(arch::PGSIZE);
        let _ = self.pages.split_off(&new_page_count);

        let tail_offset = new_size % arch::PGSIZE;
        if tail_offset != 0
            && let Some(page) = self.pages.get(&(new_page_count - 1))
        {
            page.frame.slice()[tail_offset..].fill(0);
        }
    }

    pub fn clear(&mut self) {
        self.pages.clear();
    }

    pub fn is_over_capacity(&self) -> bool {
        Self::CAPACITY != 0 && self.pages.len() > Self::CAPACITY
    }

    pub fn shrink_to_capacity(&mut self) {
        while self.pages.len() > Self::CAPACITY {
            if !self.remove_reclaimable_page() {
                break;
            }
        }
    }

    fn shrink_for_insert(&mut self) {
        while self.pages.len() >= Self::CAPACITY {
            if !self.remove_reclaimable_page() {
                break;
            }
        }
    }

    fn remove_reclaimable_page(&mut self) -> bool {
        let Some(page_index) = self
            .pages
            .iter()
            .find_map(|(page_index, page)| (!page.dirty && Arc::strong_count(&page.frame) == 1).then_some(*page_index))
        else {
            return false;
        };
        self.pages.remove(&page_index);
        true
    }
}

pub(super) struct SuperBlockInner {
    driver: Arc<dyn BlockDriverOps>,
    info: Ext4Info,
    superblock_raw: [u8; SUPERBLOCK_SIZE],
    read_only: bool,
    inode_cache: LRUCache<u32, CachedInodeRaw>,
    group_desc_cache: LRUCache<u32, GroupDescriptor>,
}

impl SuperBlockInner {
    fn new(driver: Arc<dyn BlockDriverOps>, read_only: bool) -> SysResult<Self> {
        let (info, superblock_raw) = Ext4Info::read_from(driver.clone())?;
        let mut inner = Self {
            driver,
            info,
            superblock_raw,
            read_only,
            inode_cache: LRUCache::new(),
            group_desc_cache: LRUCache::new(),
        };
        let root = inner.read_inode_raw(ROOT_INO)?;
        if root.file_type() != FileType::Directory {
            return Err(Errno::EINVAL);
        }
        Ok(inner)
    }

    pub fn is_readonly(&self) -> bool {
        self.read_only || self.driver.is_readonly()
    }

    pub fn has_inode(&mut self, ino: u32) -> bool {
        if !(1..=self.info.inodes_count).contains(&ino) {
            return false;
        }
        self.read_inode_raw(ino).is_ok()
    }

    pub fn inode_size(&mut self, ino: u32) -> SysResult<u64> {
        Ok(self.read_inode_raw(ino)?.size)
    }

    pub fn inode_cache_state(&mut self, ino: u32) -> SysResult<(bool, usize, bool)> {
        let inode = self.read_inode_raw(ino)?;
        let cacheable_file = inode.file_type() == FileType::Regular;
        let cached_size = usize::try_from(inode.size).map_err(|_| Errno::EFBIG)?;
        let fast_cached_write = cacheable_file
            && !self.is_readonly()
            && inode.flags & (EXT4_INODE_FLAG_IMMUTABLE | EXT4_INODE_FLAG_APPEND) == 0
            && self.ensure_inode_data_supported(&inode).is_ok();
        Ok((cacheable_file, cached_size, fast_cached_write))
    }

    pub fn inode_mode(&mut self, ino: u32) -> SysResult<Mode> {
        Ok(Mode::from_bits_truncate(self.read_inode_raw(ino)?.mode as u32))
    }

    pub fn inode_owner(&mut self, ino: u32) -> SysResult<(u32, u32)> {
        let inode = self.read_inode_raw(ino)?;
        Ok((inode.uid, inode.gid))
    }

    pub fn inode_type(&mut self, ino: u32) -> SysResult<FileType> {
        Ok(self.read_inode_raw(ino)?.file_type())
    }

    pub fn inode_stat(&mut self, ino: u32) -> SysResult<FileStat> {
        let inode = self.read_inode_raw(ino)?;
        let mut stat = FileStat::default();
        stat.st_ino = ino as u64;
        stat.st_mode = inode.mode as u32;
        stat.st_nlink = inode.links_count as u32;
        stat.st_uid = inode.uid;
        stat.st_gid = inode.gid;
        stat.st_size = inode.size as i64;
        stat.st_blksize = self.info.block_size as i32;
        stat.st_blocks = inode.blocks_count;
        stat.st_atime_sec = inode.atime as i64;
        stat.st_mtime_sec = inode.mtime as i64;
        stat.st_ctime_sec = inode.ctime as i64;
        Ok(stat)
    }

    pub fn lookup(&mut self, ino: u32, name: &str, page_cache: &mut InodePageCache) -> SysResult<u32> {
        let inode = self.read_inode_raw(ino)?;
        if inode.file_type() != FileType::Directory {
            return Err(Errno::ENOTDIR);
        }
        let _ = page_cache;
        self.find_directory_entry(&inode, name)?
            .map(|entry| entry.ino)
            .ok_or(Errno::ENOENT)
    }

    pub fn get_dent(
        &mut self,
        ino: u32,
        index: usize,
        page_cache: &mut InodePageCache,
    ) -> SysResult<Option<(DirResult, usize)>> {
        let inode = self.read_inode_raw(ino)?;
        if inode.file_type() != FileType::Directory {
            return Err(Errno::ENOTDIR);
        }

        let Some(entry) = self.directory_entries(&inode, Some(page_cache))?.into_iter().nth(index) else {
            return Ok(None);
        };
        Ok(Some((
            DirResult {
                ino: entry.ino,
                name: entry.name,
                file_type: entry.file_type,
            },
            index + 1,
        )))
    }

    pub fn read_inode(
        &mut self,
        ino: u32,
        buf: &mut [u8],
        offset: usize,
        page_cache: Option<&mut InodePageCache>,
    ) -> SysResult<usize> {
        let inode = self.read_inode_raw(ino)?;
        if inode.file_type() == FileType::Directory {
            return Err(Errno::EISDIR);
        }
        self.read_inode_data(&inode, buf, offset, page_cache)
    }

    pub fn write_inode(&mut self, ino: u32, buf: &[u8], offset: usize) -> SysResult<usize> {
        if buf.is_empty() {
            return Ok(0);
        }
        if self.is_readonly() {
            return Err(Errno::EROFS);
        }

        let mut inode = self.read_inode_raw(ino)?;
        match inode.file_type() {
            FileType::Regular => {}
            FileType::Directory => return Err(Errno::EISDIR),
            _ => return Err(Errno::EOPNOTSUPP),
        }
        if inode.flags & EXT4_INODE_FLAG_IMMUTABLE != 0 {
            return Err(Errno::EPERM);
        }
        if inode.flags & EXT4_INODE_FLAG_APPEND != 0 && offset as u64 != inode.size {
            return Err(Errno::EPERM);
        }
        self.ensure_inode_data_supported(&inode)?;

        let written = self.write_inode_data(&mut inode, buf, offset)?;
        if written == 0 {
            return Ok(0);
        }

        let new_size = cmp::max(inode.size, offset.checked_add(written).ok_or(Errno::EFBIG)? as u64);
        inode.set_size(new_size);
        let now = now();
        inode.set_mtime(&now);
        inode.set_ctime(&now);
        self.mark_inode_dirty(ino, inode)?;
        Ok(written)
    }

    pub fn writeback_inode(&mut self, ino: u32, buf: &[u8], offset: usize) -> SysResult<usize> {
        if buf.is_empty() {
            return Ok(0);
        }
        if self.is_readonly() {
            return Err(Errno::EROFS);
        }

        let mut inode = self.read_inode_raw(ino)?;
        if inode.file_type() != FileType::Regular {
            return Err(Errno::EOPNOTSUPP);
        }
        self.ensure_inode_data_supported(&inode)?;

        let written = self.write_inode_data(&mut inode, buf, offset)?;
        if written == 0 {
            return Ok(0);
        }

        let new_size = cmp::max(inode.size, offset.checked_add(written).ok_or(Errno::EFBIG)? as u64);
        inode.set_size(new_size);
        self.mark_inode_dirty(ino, inode)?;
        Ok(written)
    }

    pub fn prepare_cached_write(
        &mut self,
        ino: u32,
        offset: usize,
        len: usize,
        cached_size: usize,
    ) -> SysResult<usize> {
        let end = offset.checked_add(len).ok_or(Errno::EFBIG)?;
        if end > u32::MAX as usize * self.info.block_size as usize {
            return Err(Errno::EFBIG);
        }
        if self.is_readonly() {
            return Err(Errno::EROFS);
        }

        let inode = self.read_inode_raw(ino)?;
        match inode.file_type() {
            FileType::Regular => {}
            FileType::Directory => return Err(Errno::EISDIR),
            _ => return Err(Errno::EOPNOTSUPP),
        }
        if inode.flags & EXT4_INODE_FLAG_IMMUTABLE != 0 {
            return Err(Errno::EPERM);
        }
        if inode.flags & EXT4_INODE_FLAG_APPEND != 0 && offset != cached_size {
            return Err(Errno::EPERM);
        }
        self.ensure_inode_data_supported(&inode)?;
        Ok(cached_size)
    }

    pub fn update_inode_times(
        &mut self,
        ino: u32,
        atime: Option<&Duration>,
        mtime: Option<&Duration>,
        ctime: Option<&Duration>,
    ) -> SysResult<()> {
        if self.is_readonly() {
            return Err(Errno::EROFS);
        }

        let mut inode = self.read_inode_raw(ino)?;
        if let Some(time) = atime {
            inode.set_atime(time);
        }
        if let Some(time) = mtime {
            inode.set_mtime(time);
        }
        if let Some(time) = ctime {
            inode.set_ctime(time);
        }
        self.mark_inode_dirty(ino, inode)
    }

    pub fn flush_inode(&mut self, ino: u32) -> SysResult<()> {
        if !self.is_readonly() {
            self.flush_inode_metadata(ino)?;
        }
        self.flush_driver()
    }

    pub fn sync(&mut self) -> SysResult<()> {
        if !self.is_readonly() {
            self.flush_dirty_inode_metadata()?;
        }
        self.flush_driver()
    }

    fn flush_driver(&self) -> SysResult<()> {
        self.driver.flush().map_err(|_| Errno::EIO)
    }

    pub fn readlink(&mut self, ino: u32, buf: &mut [u8], page_cache: &mut InodePageCache) -> SysResult<Option<usize>> {
        let inode = self.read_inode_raw(ino)?;
        if inode.file_type() != FileType::Symlink {
            return Ok(None);
        }

        let len = cmp::min(buf.len(), usize::try_from(inode.size).map_err(|_| Errno::EFBIG)?);
        if inode.blocks_count == 0 && inode.size <= inode.block.len() as u64 {
            buf[..len].copy_from_slice(&inode.block[..len]);
            return Ok(Some(len));
        }

        let mut target = Vec::new();
        target.resize(len, 0);
        let read = self.read_inode_data(&inode, &mut target, 0, Some(page_cache))?;
        buf[..read].copy_from_slice(&target[..read]);
        Ok(Some(read))
    }

    pub fn create_child(&mut self, parent_ino: u32, name: &str, mode: Mode, owner: Owner, dev: u64) -> SysResult<u32> {
        if self.is_readonly() {
            return Err(Errno::EROFS);
        }
        Self::validate_filename(name)?;

        let mut parent = self.read_inode_raw(parent_ino)?;
        self.ensure_directory_mutable(&parent)?;
        if self.find_directory_entry(&parent, name)?.is_some() {
            return Err(Errno::EEXIST);
        }

        let file_type: FileType = mode.into();
        if file_type == FileType::Unknown {
            return Err(Errno::EINVAL);
        }
        let links = if file_type == FileType::Directory { 2 } else { 1 };
        let mut child = self.allocate_inode(mode, owner, links)?;
        let child_ino = child.ino;
        let result = (|| {
            match file_type {
                FileType::Directory => self.init_directory_inode(&mut child, parent_ino)?,
                FileType::CharDevice | FileType::BlockDevice => child.set_dev(dev as u32),
                FileType::FIFO | FileType::Socket | FileType::Regular | FileType::Symlink => {}
                FileType::Unknown => return Err(Errno::EINVAL),
            }

            self.add_directory_entry(&mut parent, name, child_ino, file_type)?;
            let time = now();
            parent.set_mtime(&time);
            parent.set_ctime(&time);
            if file_type == FileType::Directory {
                parent.increment_links_count()?;
            }
            child.set_ctime(&time);
            self.mark_inode_dirty(parent_ino, parent)?;
            self.mark_inode_dirty(child_ino, child.clone())?;
            Ok(())
        })();

        if let Err(err) = result {
            let _ = self.free_inode(child_ino, child);
            return Err(err);
        }
        Ok(child_ino)
    }

    pub fn link_child(&mut self, parent_ino: u32, name: &str, target_ino: u32) -> SysResult<()> {
        if self.is_readonly() {
            return Err(Errno::EROFS);
        }
        let mut parent = self.read_inode_raw(parent_ino)?;
        self.ensure_directory_mutable(&parent)?;
        if self.find_directory_entry(&parent, name)?.is_some() {
            return Err(Errno::EEXIST);
        }

        let mut target = self.read_inode_raw(target_ino)?;
        if target.file_type() == FileType::Directory {
            return Err(Errno::EPERM);
        }
        if target.flags & EXT4_INODE_FLAG_IMMUTABLE != 0 {
            return Err(Errno::EPERM);
        }

        self.add_directory_entry(&mut parent, name, target_ino, target.file_type())?;
        let time = now();
        parent.set_mtime(&time);
        parent.set_ctime(&time);
        target.increment_links_count()?;
        target.set_ctime(&time);
        self.mark_inode_dirty(parent_ino, parent)?;
        self.mark_inode_dirty(target_ino, target)
    }

    pub fn unlink_child(&mut self, parent_ino: u32, name: &str) -> SysResult<()> {
        if self.is_readonly() {
            return Err(Errno::EROFS);
        }
        let mut parent = self.read_inode_raw(parent_ino)?;
        self.ensure_directory_mutable(&parent)?;
        let location = self.find_directory_entry(&parent, name)?.ok_or(Errno::ENOENT)?;
        let child_ino = location.ino;
        let child_type = location.file_type;
        let mut child = self.read_inode_raw(child_ino)?;
        let time = now();

        if child_type == FileType::Directory {
            if !self.is_empty_directory(&child)? {
                return Err(Errno::ENOTEMPTY);
            }
            parent.decrement_links_count()?;
            child.set_links_count(0);
        } else {
            child.decrement_links_count()?;
        }

        self.remove_directory_entry(&mut parent, name, Some(child_type))?;
        parent.set_mtime(&time);
        parent.set_ctime(&time);
        child.set_ctime(&time);
        self.mark_inode_dirty(parent_ino, parent)?;
        if child.links_count == 0 {
            self.free_inode(child_ino, child)
        } else {
            self.mark_inode_dirty(child_ino, child)
        }
    }

    pub fn rename_child(
        &mut self,
        old_parent_ino: u32,
        old_name: &str,
        new_parent_ino: u32,
        new_name: &str,
    ) -> SysResult<()> {
        if self.is_readonly() {
            return Err(Errno::EROFS);
        }
        Self::validate_filename(old_name)?;
        Self::validate_filename(new_name)?;
        if old_parent_ino == new_parent_ino && old_name == new_name {
            return Ok(());
        }
        if old_parent_ino == new_parent_ino {
            return self.rename_child_same_parent(old_parent_ino, old_name, new_name);
        }

        let mut old_parent = self.read_inode_raw(old_parent_ino)?;
        self.ensure_directory_mutable(&old_parent)?;
        let old_location = self.find_directory_entry(&old_parent, old_name)?.ok_or(Errno::ENOENT)?;
        let mut source = self.read_inode_raw(old_location.ino)?;
        let source_type = source.file_type();
        let same_parent = old_parent_ino == new_parent_ino;
        let mut new_parent = if same_parent {
            old_parent.clone()
        } else {
            let parent = self.read_inode_raw(new_parent_ino)?;
            self.ensure_directory_mutable(&parent)?;
            parent
        };

        if let Some(target_location) = self.find_directory_entry(&new_parent, new_name)? {
            if target_location.ino == source.ino {
                self.remove_directory_entry(&mut old_parent, old_name, None)?;
                let time = now();
                old_parent.set_mtime(&time);
                old_parent.set_ctime(&time);
                self.mark_inode_dirty(old_parent_ino, old_parent)?;
                return Ok(());
            }

            let mut target = self.read_inode_raw(target_location.ino)?;
            let target_type = target.file_type();
            match (source_type, target_type) {
                (FileType::Directory, FileType::Directory) => {
                    if !self.is_empty_directory(&target)? {
                        return Err(Errno::ENOTEMPTY);
                    }
                }
                (FileType::Directory, _) => return Err(Errno::ENOTDIR),
                (_, FileType::Directory) => return Err(Errno::EISDIR),
                _ => {}
            }

            self.remove_directory_entry(&mut new_parent, new_name, Some(target_type))?;
            target.decrement_links_count()?;
            if target.links_count == 0 {
                self.free_inode(target.ino, target)?;
            } else {
                target.set_ctime(&now());
                self.mark_inode_dirty(target.ino, target)?;
            }
        }

        self.remove_directory_entry(&mut old_parent, old_name, Some(source_type))?;
        self.add_directory_entry(&mut new_parent, new_name, source.ino, source_type)?;
        let time = now();
        old_parent.set_mtime(&time);
        old_parent.set_ctime(&time);
        new_parent.set_mtime(&time);
        new_parent.set_ctime(&time);
        source.set_ctime(&time);

        if source_type == FileType::Directory && old_parent_ino != new_parent_ino {
            old_parent.decrement_links_count()?;
            new_parent.increment_links_count()?;
            self.update_dotdot(&mut source, new_parent_ino)?;
        }

        if same_parent {
            self.mark_inode_dirty(old_parent_ino, new_parent)?;
        } else {
            self.mark_inode_dirty(old_parent_ino, old_parent)?;
            self.mark_inode_dirty(new_parent_ino, new_parent)?;
        }
        self.mark_inode_dirty(source.ino, source)
    }

    pub fn set_symlink(&mut self, ino: u32, target: &str) -> SysResult<()> {
        if self.is_readonly() {
            return Err(Errno::EROFS);
        }
        let mut inode = self.read_inode_raw(ino)?;
        if inode.file_type() != FileType::Symlink {
            return Err(Errno::EINVAL);
        }
        if target.len() <= inode.block.len() {
            self.free_inode_data(&mut inode)?;
            inode.set_fast_symlink(target.as_bytes());
        } else {
            inode.set_extent_root();
            let written = self.write_inode_data(&mut inode, target.as_bytes(), 0)?;
            if written != target.len() {
                return Err(Errno::EIO);
            }
            inode.set_size(target.len() as u64);
        }
        let time = now();
        inode.set_mtime(&time);
        inode.set_ctime(&time);
        self.mark_inode_dirty(ino, inode)
    }

    pub fn chmod_inode(&mut self, ino: u32, mode: Mode) -> SysResult<()> {
        if self.is_readonly() {
            return Err(Errno::EROFS);
        }
        let mut inode = self.read_inode_raw(ino)?;
        let current = inode.mode as u32;
        inode.set_mode(((current & !0o7777) | (mode.bits() & 0o7777)) as u16);
        inode.set_ctime(&now());
        self.mark_inode_dirty(ino, inode)
    }

    pub fn chown_inode(&mut self, ino: u32, uid: Option<u32>, gid: Option<u32>) -> SysResult<()> {
        if self.is_readonly() {
            return Err(Errno::EROFS);
        }
        let mut inode = self.read_inode_raw(ino)?;
        inode.set_owner(uid.unwrap_or(inode.uid), gid.unwrap_or(inode.gid));
        let mut mode = inode.mode as u32;
        if inode.file_type() != FileType::Directory {
            if mode & Mode::S_IXGRP.bits() != 0 {
                mode &= !(Mode::S_ISUID | Mode::S_ISGID).bits();
            } else {
                mode &= !Mode::S_ISUID.bits();
            }
            inode.set_mode(mode as u16);
        }
        inode.set_ctime(&now());
        self.mark_inode_dirty(ino, inode)
    }

    pub fn truncate_inode(&mut self, ino: u32, new_size: u64) -> SysResult<()> {
        if self.is_readonly() {
            return Err(Errno::EROFS);
        }
        let mut inode = self.read_inode_raw(ino)?;
        if inode.file_type() == FileType::Directory {
            return Err(Errno::EISDIR);
        }
        if inode.flags & (EXT4_INODE_FLAG_IMMUTABLE | EXT4_INODE_FLAG_APPEND) != 0 {
            return Err(Errno::EPERM);
        }
        if new_size < inode.size {
            self.truncate_inode_data(&mut inode, new_size)?;
        } else if new_size > inode.size {
            self.ensure_inode_data_supported(&inode)?;
            inode.set_size(new_size);
        }
        let time = now();
        inode.set_mtime(&time);
        inode.set_ctime(&time);
        self.mark_inode_dirty(ino, inode)
    }

    fn read_inode_data(
        &self,
        inode: &Ext4InodeRaw,
        buf: &mut [u8],
        offset: usize,
        page_cache: Option<&mut InodePageCache>,
    ) -> SysResult<usize> {
        if buf.is_empty() {
            return Ok(0);
        }

        let read_len = self.read_len(inode, buf.len(), offset)?;
        if read_len == 0 {
            return Ok(0);
        }

        self.ensure_inode_data_supported(inode)?;

        if let Some(page_cache) = page_cache {
            return self.read_inode_data_cached(inode, &mut buf[..read_len], offset, page_cache);
        }

        self.read_inode_data_uncached(inode, &mut buf[..read_len], offset)
    }

    fn read_inode_data_cached(
        &self,
        inode: &Ext4InodeRaw,
        buf: &mut [u8],
        offset: usize,
        page_cache: &mut InodePageCache,
    ) -> SysResult<usize> {
        let mut copied = 0usize;
        while copied < buf.len() {
            let current = offset.checked_add(copied).ok_or(Errno::EFBIG)?;
            let page_index = current / arch::PGSIZE;
            let page_offset = current % arch::PGSIZE;
            let copy_len = cmp::min(buf.len() - copied, arch::PGSIZE - page_offset);
            let frame = self.read_inode_page(inode, page_index, page_cache)?;
            frame.copy_to_slice(page_offset, &mut buf[copied..copied + copy_len]);

            copied += copy_len;
        }

        Ok(copied)
    }

    fn read_inode_data_uncached(&self, inode: &Ext4InodeRaw, buf: &mut [u8], offset: usize) -> SysResult<usize> {
        let read_len = self.read_len(inode, buf.len(), offset)?;
        let mut copied = 0usize;
        let mut current = offset as u64;
        while copied < read_len {
            let logical_block = current / self.info.block_size;
            let block_offset = current % self.info.block_size;
            let copy_len = cmp::min(read_len - copied, (self.info.block_size - block_offset) as usize);

            match self.map_extent_block(inode, u32::try_from(logical_block).map_err(|_| Errno::EFBIG)?)? {
                BlockMapping::Data(block) => {
                    let device_offset = block
                        .checked_mul(self.info.block_size)
                        .and_then(|offset| offset.checked_add(block_offset))
                        .ok_or(Errno::EIO)?;
                    self.read_at(device_offset, &mut buf[copied..copied + copy_len])?;
                }
                BlockMapping::Hole | BlockMapping::Unwritten => {
                    buf[copied..copied + copy_len].fill(0);
                }
            }

            copied += copy_len;
            current += copy_len as u64;
        }

        Ok(copied)
    }

    fn write_inode_data(&mut self, inode: &mut Ext4InodeRaw, buf: &[u8], offset: usize) -> SysResult<usize> {
        let end = offset.checked_add(buf.len()).ok_or(Errno::EFBIG)?;
        if end > u32::MAX as usize * self.info.block_size as usize {
            return Err(Errno::EFBIG);
        }

        let block_size = usize::try_from(self.info.block_size).map_err(|_| Errno::EIO)?;
        let old_size = inode.size;
        let original_append_end = self.append_logical_end(inode)?;
        let mut appended_until = original_append_end;
        let mut written = 0usize;

        while written < buf.len() {
            let current = offset.checked_add(written).ok_or(Errno::EFBIG)?;
            let logical_block = u32::try_from(current / block_size).map_err(|_| Errno::EFBIG)?;
            let block_offset = current % block_size;
            let copy_len = cmp::min(buf.len() - written, block_size - block_offset);
            let needs_append = logical_block >= appended_until;
            let was_appended = logical_block >= original_append_end;

            if needs_append {
                let last_write_block = u32::try_from((end - 1) / block_size).map_err(|_| Errno::EFBIG)?;
                let needed = last_write_block
                    .checked_add(1)
                    .and_then(|end_block| end_block.checked_sub(appended_until))
                    .ok_or(Errno::EFBIG)?;
                self.allocate_append_blocks(inode, appended_until, needed)?;
                appended_until = appended_until.checked_add(needed).ok_or(Errno::EFBIG)?;
            }

            let pblock = match self.map_extent_block(inode, logical_block)? {
                BlockMapping::Data(block) => block,
                BlockMapping::Hole | BlockMapping::Unwritten => {
                    if written == 0 {
                        return Err(Errno::EOPNOTSUPP);
                    }
                    return Ok(written);
                }
            };

            let device_offset = pblock.checked_mul(self.info.block_size).ok_or(Errno::EIO)?;
            let full_block_write =
                block_offset == 0 && copy_len == block_size && (current as u64 <= old_size || was_appended);
            if full_block_write {
                self.write_at(device_offset, &buf[written..written + copy_len])?;
            } else {
                let mut block = Vec::new();
                block.resize(block_size, 0);
                self.read_at(device_offset, &mut block)?;

                let block_start = (logical_block as u64)
                    .checked_mul(self.info.block_size)
                    .ok_or(Errno::EIO)?;
                if old_size > block_start && old_size < current as u64 {
                    let zero_start = usize::try_from(old_size - block_start).map_err(|_| Errno::EIO)?;
                    block[zero_start..block_offset].fill(0);
                } else if old_size <= block_start && current as u64 > block_start {
                    block[..block_offset].fill(0);
                }

                block[block_offset..block_offset + copy_len].copy_from_slice(&buf[written..written + copy_len]);
                self.write_at(device_offset, &block)?;
            }

            written += copy_len;
        }

        Ok(written)
    }

    fn read_inode_page(
        &self,
        inode: &Ext4InodeRaw,
        page_index: usize,
        page_cache: &mut InodePageCache,
    ) -> SysResult<Arc<PhysPageFrame>> {
        if let Some(frame) = page_cache.get_frame(page_index) {
            return Ok(frame);
        }

        let page_offset = page_index.checked_mul(arch::PGSIZE).ok_or(Errno::EFBIG)?;
        let frame = Arc::new(PhysPageFrame::alloc());
        let read_len = self.read_len(inode, arch::PGSIZE, page_offset)?;
        {
            let page = frame.slice();
            if read_len > 0 {
                self.read_inode_data_uncached(inode, &mut page[..read_len], page_offset)?;
            }
            page[read_len..].fill(0);
        }

        Ok(page_cache.insert_frame(page_index, frame))
    }

    fn read_len(&self, inode: &Ext4InodeRaw, buf_len: usize, offset: usize) -> SysResult<usize> {
        let offset = offset as u64;
        if offset >= inode.size {
            return Ok(0);
        }
        Ok(cmp::min(buf_len as u64, inode.size - offset) as usize)
    }

    fn ensure_inode_data_supported(&self, inode: &Ext4InodeRaw) -> SysResult<()> {
        if inode.flags & EXT4_INLINE_DATA_FL != 0 {
            return Err(Errno::EOPNOTSUPP);
        }
        if inode.flags & EXT4_INODE_FLAG_EXTENTS == 0 {
            return Err(Errno::EOPNOTSUPP);
        }
        Ok(())
    }

    fn ensure_linear_directory_supported(&self, inode: &Ext4InodeRaw) -> SysResult<()> {
        self.ensure_inode_data_supported(inode)?;
        if inode.flags & EXT4_INODE_FLAG_INDEX != 0 {
            return Err(Errno::EOPNOTSUPP);
        }
        Ok(())
    }

    fn directory_entries(
        &mut self,
        inode: &Ext4InodeRaw,
        mut page_cache: Option<&mut InodePageCache>,
    ) -> SysResult<Vec<DirectoryEntry>> {
        let mut entries = Vec::new();
        self.ensure_linear_directory_supported(inode)?;

        let block_count = inode.size.div_ceil(self.info.block_size);
        for logical_block in 0..block_count {
            let block_offset = logical_block.checked_mul(self.info.block_size).ok_or(Errno::EFBIG)?;
            let block_offset = usize::try_from(block_offset).map_err(|_| Errno::EFBIG)?;
            let mut block = Vec::new();
            block.resize(self.info.block_size as usize, 0);
            let block_len = self.read_inode_data(
                inode,
                &mut block,
                block_offset,
                page_cache.as_mut().map(|cache| &mut **cache),
            )?;
            if block_len == 0 {
                break;
            }
            let block = &block[..block_len];
            let mut offset = 0usize;
            while offset < block.len() {
                let remaining = block.len() - offset;
                if remaining < EXT4_DIR_ENTRY_HEADER_SIZE {
                    break;
                }

                let child_ino = le_u32(&block, offset);
                let entry_len = le_u16(&block, offset + 4) as usize;
                let name_len = block[offset + 6] as usize;
                let dirent_type = block[offset + 7];

                if entry_len == 0 {
                    return Err(Errno::EIO);
                }
                if entry_len < EXT4_DIR_ENTRY_HEADER_SIZE || entry_len > remaining {
                    return Err(Errno::EIO);
                }
                if name_len > entry_len - EXT4_DIR_ENTRY_HEADER_SIZE || name_len > EXT4_NAME_LEN {
                    return Err(Errno::EIO);
                }

                if child_ino != 0 {
                    let name_start = offset + EXT4_DIR_ENTRY_HEADER_SIZE;
                    let name_end = name_start + name_len;
                    let name = core::str::from_utf8(&block[name_start..name_end]).map_err(|_| Errno::EIO)?;
                    entries.push(DirectoryEntry {
                        ino: child_ino,
                        name: String::from(name),
                        file_type: self.dirent_file_type(dirent_type, child_ino)?,
                    });
                }

                offset += entry_len;
            }
        }

        Ok(entries)
    }

    fn dirent_file_type(&mut self, dirent_type: u8, ino: u32) -> SysResult<FileType> {
        if !self.info.features_incompatible.contains(Ext4IncompatFeatures::FILETYPE) {
            return Ok(self.read_inode_raw(ino)?.file_type());
        }

        Ok(match dirent_type {
            EXT4_DE_REG_FILE => FileType::Regular,
            EXT4_DE_DIR => FileType::Directory,
            EXT4_DE_SYMLINK => FileType::Symlink,
            EXT4_DE_CHRDEV => FileType::CharDevice,
            EXT4_DE_BLKDEV => FileType::BlockDevice,
            EXT4_DE_FIFO => FileType::FIFO,
            EXT4_DE_SOCK => FileType::Socket,
            EXT4_DE_UNKNOWN => FileType::Unknown,
            _ => FileType::Unknown,
        })
    }

    fn validate_filename(name: &str) -> SysResult<()> {
        if name.is_empty() || name == "." || name == ".." || name.as_bytes().contains(&b'/') {
            return Err(Errno::EINVAL);
        }
        if name.len() > EXT4_NAME_LEN {
            return Err(Errno::ENAMETOOLONG);
        }
        Ok(())
    }

    fn dirent_type(file_type: FileType) -> u8 {
        match file_type {
            FileType::Regular => EXT4_DE_REG_FILE,
            FileType::Directory => EXT4_DE_DIR,
            FileType::Symlink => EXT4_DE_SYMLINK,
            FileType::CharDevice => EXT4_DE_CHRDEV,
            FileType::BlockDevice => EXT4_DE_BLKDEV,
            FileType::FIFO => EXT4_DE_FIFO,
            FileType::Socket => EXT4_DE_SOCK,
            FileType::Unknown => EXT4_DE_UNKNOWN,
        }
    }

    fn find_directory_entry(&mut self, inode: &Ext4InodeRaw, name: &str) -> SysResult<Option<DirectoryEntryLocation>> {
        if inode.file_type() != FileType::Directory {
            return Err(Errno::ENOTDIR);
        }
        self.ensure_inode_data_supported(inode)?;
        if inode.flags & EXT4_INODE_FLAG_INDEX != 0 {
            return self.find_indexed_directory_entry(inode, name);
        }

        self.find_linear_directory_entry(inode, name, None)
    }

    fn find_linear_directory_entry(
        &mut self,
        inode: &Ext4InodeRaw,
        name: &str,
        only_logical_block: Option<u32>,
    ) -> SysResult<Option<DirectoryEntryLocation>> {
        let block_size = self.info.block_size as usize;
        let first_block = only_logical_block.map_or(0, u64::from);
        let block_count = only_logical_block.map_or(inode.size.div_ceil(self.info.block_size), |block| {
            u64::from(block).saturating_add(1)
        });
        for logical_block in first_block..block_count {
            let block_offset = logical_block.checked_mul(self.info.block_size).ok_or(Errno::EFBIG)?;
            let block_offset = usize::try_from(block_offset).map_err(|_| Errno::EFBIG)?;
            let mut block = Vec::new();
            block.resize(block_size, 0);
            let block_len = self.read_inode_data(inode, &mut block, block_offset, None)?;
            if block_len == 0 {
                break;
            }

            let mut offset = 0usize;
            let mut previous_entry_offset = None;
            while offset < block_len {
                let remaining = block_len - offset;
                if remaining < EXT4_DIR_ENTRY_HEADER_SIZE {
                    return Err(Errno::EIO);
                }
                let child_ino = le_u32(&block, offset);
                let entry_len = le_u16(&block, offset + 4) as usize;
                let name_len = block[offset + 6] as usize;
                let dirent_type = block[offset + 7];
                if entry_len == 0
                    || entry_len < EXT4_DIR_ENTRY_HEADER_SIZE
                    || entry_len > remaining
                    || name_len > entry_len - EXT4_DIR_ENTRY_HEADER_SIZE
                    || name_len > EXT4_NAME_LEN
                {
                    return Err(Errno::EIO);
                }

                let name_start = offset + EXT4_DIR_ENTRY_HEADER_SIZE;
                let name_end = name_start + name_len;
                if child_ino != 0 && block[name_start..name_end] == *name.as_bytes() {
                    return Ok(Some(DirectoryEntryLocation {
                        block,
                        block_offset,
                        previous_entry_offset,
                        entry_offset: offset,
                        entry_len,
                        ino: child_ino,
                        file_type: self.dirent_file_type(dirent_type, child_ino)?,
                    }));
                }

                previous_entry_offset = Some(offset);
                offset += entry_len;
            }
        }

        Ok(None)
    }

    fn find_indexed_directory_entry(
        &mut self,
        inode: &Ext4InodeRaw,
        name: &str,
    ) -> SysResult<Option<DirectoryEntryLocation>> {
        if let Some(location) = self.find_linear_directory_entry(inode, name, Some(0))? {
            return Ok(Some(location));
        }

        let root_block = self.read_directory_logical_block(inode, 0)?;
        let (hash_version, root, indirect_levels) = self.parse_htree_root(&root_block)?;
        let hash = htree_hash(name.as_bytes(), self.info.hash_seed, hash_version)?;
        let root_start = htree_find_position(&root_block, root, hash);

        match indirect_levels {
            0 => {
                let mut pos = root_start;
                while pos < root.count as usize {
                    if pos != root_start && !htree_hash_matches(&root_block, root, pos, hash) {
                        break;
                    }
                    let logical_block = htree_entry_block(&root_block, root, pos)?;
                    if let Some(location) = self.find_linear_directory_entry(inode, name, Some(logical_block))? {
                        return Ok(Some(location));
                    }
                    pos += 1;
                }
            }
            1 => {
                let mut root_pos = root_start;
                while root_pos < root.count as usize {
                    if root_pos != root_start && !htree_hash_matches(&root_block, root, root_pos, hash) {
                        break;
                    }

                    let node_logical_block = htree_entry_block(&root_block, root, root_pos)?;
                    let node_block = self.read_directory_logical_block(inode, node_logical_block)?;
                    let node = self.parse_htree_node(&node_block)?;
                    let node_start = if root_pos == root_start {
                        htree_find_position(&node_block, node, hash)
                    } else {
                        0
                    };

                    let mut node_pos = node_start;
                    while node_pos < node.count as usize {
                        if node_pos != node_start && !htree_hash_matches(&node_block, node, node_pos, hash) {
                            break;
                        }
                        let logical_block = htree_entry_block(&node_block, node, node_pos)?;
                        if let Some(location) = self.find_linear_directory_entry(inode, name, Some(logical_block))? {
                            return Ok(Some(location));
                        }
                        node_pos += 1;
                    }

                    root_pos += 1;
                }
            }
            _ => return Err(Errno::EOPNOTSUPP),
        }

        Ok(None)
    }

    fn find_indexed_directory_leaf(&self, inode: &Ext4InodeRaw, hash: u32) -> SysResult<HTreeLeafPath> {
        let root_block = self.read_directory_logical_block(inode, 0)?;
        let (_, root, indirect_levels) = self.parse_htree_root(&root_block)?;
        let root_position = htree_find_position(&root_block, root, hash);

        match indirect_levels {
            0 => {
                let leaf_logical_block = htree_entry_block(&root_block, root, root_position)?;
                Ok(HTreeLeafPath {
                    index_block: root_block,
                    index_logical_block: 0,
                    index_node: root,
                    index_position: root_position,
                    leaf_logical_block,
                })
            }
            1 => {
                let node_logical_block = htree_entry_block(&root_block, root, root_position)?;
                let node_block = self.read_directory_logical_block(inode, node_logical_block)?;
                let node = self.parse_htree_node(&node_block)?;
                let node_position = htree_find_position(&node_block, node, hash);
                let leaf_logical_block = htree_entry_block(&node_block, node, node_position)?;
                Ok(HTreeLeafPath {
                    index_block: node_block,
                    index_logical_block: node_logical_block,
                    index_node: node,
                    index_position: node_position,
                    leaf_logical_block,
                })
            }
            _ => Err(Errno::EOPNOTSUPP),
        }
    }

    fn read_directory_logical_block(&self, inode: &Ext4InodeRaw, logical_block: u32) -> SysResult<Vec<u8>> {
        let block_size = self.info.block_size as usize;
        let block_offset = u64::from(logical_block)
            .checked_mul(self.info.block_size)
            .ok_or(Errno::EFBIG)?;
        let block_offset = usize::try_from(block_offset).map_err(|_| Errno::EFBIG)?;
        let mut block = Vec::new();
        block.resize(block_size, 0);
        let block_len = self.read_inode_data(inode, &mut block, block_offset, None)?;
        if block_len != block_size {
            return Err(Errno::EIO);
        }
        Ok(block)
    }

    fn parse_htree_root(&self, block: &[u8]) -> SysResult<(HTreeHashVersion, HTreeNode, u8)> {
        let block_size = self.info.block_size as usize;
        if block.len() != block_size
            || le_u16(block, 4) != EXT4_DX_ROOT_INFO_OFFSET as u16 - 12
            || le_u16(block, 16) as usize != block_size - 12
            || le_u32(block, EXT4_DX_ROOT_INFO_OFFSET) != 0
            || block[EXT4_DX_ROOT_INFO_OFFSET + 5] != 8
            || block[EXT4_DX_ROOT_INFO_OFFSET + 7] != 0
        {
            return Err(Errno::EIO);
        }

        let hash_version = HTreeHashVersion::from_raw(block[EXT4_DX_ROOT_INFO_OFFSET + 4], self.info.flags)?;
        let indirect_levels = block[EXT4_DX_ROOT_INFO_OFFSET + 6];
        if indirect_levels > 1 {
            return Err(Errno::EOPNOTSUPP);
        }

        let entry_space = block_size
            .checked_sub(EXT4_DX_ROOT_ENTRY_OFFSET)
            .and_then(|space| space.checked_sub(self.htree_tail_size()))
            .ok_or(Errno::EIO)?;
        let expected_limit = entry_space / EXT4_DX_ENTRY_SIZE;
        let node = self.parse_htree_entries(block, EXT4_DX_ROOT_ENTRY_OFFSET, expected_limit)?;
        Ok((hash_version, node, indirect_levels))
    }

    fn parse_htree_node(&self, block: &[u8]) -> SysResult<HTreeNode> {
        let block_size = self.info.block_size as usize;
        if block.len() != block_size || le_u16(block, 4) as usize != block_size {
            return Err(Errno::EIO);
        }
        let entry_space = block_size
            .checked_sub(EXT4_DX_NODE_ENTRY_OFFSET)
            .and_then(|space| space.checked_sub(self.htree_tail_size()))
            .ok_or(Errno::EIO)?;
        let expected_limit = entry_space / EXT4_DX_ENTRY_SIZE;
        self.parse_htree_entries(block, EXT4_DX_NODE_ENTRY_OFFSET, expected_limit)
    }

    fn parse_htree_entries(&self, block: &[u8], entry_offset: usize, expected_limit: usize) -> SysResult<HTreeNode> {
        if entry_offset + 4 > block.len() || expected_limit > u16::MAX as usize {
            return Err(Errno::EIO);
        }
        let limit = le_u16(block, entry_offset);
        let count = le_u16(block, entry_offset + 2);
        if limit as usize != expected_limit || count == 0 || count > limit {
            return Err(Errno::EIO);
        }
        let entries_end = entry_offset
            .checked_add(limit as usize * EXT4_DX_ENTRY_SIZE)
            .ok_or(Errno::EIO)?;
        if entries_end > block.len() {
            return Err(Errno::EIO);
        }
        Ok(HTreeNode {
            entry_offset,
            limit,
            count,
        })
    }

    fn htree_tail_size(&self) -> usize {
        if self.metadata_csum_enabled() { 8 } else { 0 }
    }

    fn add_directory_entry(
        &mut self,
        parent: &mut Ext4InodeRaw,
        name: &str,
        child_ino: u32,
        child_type: FileType,
    ) -> SysResult<()> {
        Self::validate_filename(name)?;
        if parent.file_type() != FileType::Directory {
            return Err(Errno::ENOTDIR);
        }
        self.ensure_inode_data_supported(parent)?;
        if self.find_directory_entry(parent, name)?.is_some() {
            return Err(Errno::EEXIST);
        }
        if parent.flags & EXT4_INODE_FLAG_INDEX != 0 {
            return self.add_indexed_directory_entry(parent, name, child_ino, child_type);
        }

        let block_size = self.info.block_size as usize;
        let block_count = parent.size.div_ceil(self.info.block_size);
        for logical_block in 0..block_count {
            let block_offset = logical_block.checked_mul(self.info.block_size).ok_or(Errno::EFBIG)?;
            let block_offset = usize::try_from(block_offset).map_err(|_| Errno::EFBIG)?;
            let mut block = Vec::new();
            block.resize(block_size, 0);
            let block_len = self.read_inode_data(parent, &mut block, block_offset, None)?;
            if block_len == 0 {
                break;
            }

            if self.try_insert_directory_entry(
                parent,
                &mut block,
                block_len,
                block_offset,
                name,
                child_ino,
                child_type,
            )? {
                return Ok(());
            }
        }

        if parent.size % self.info.block_size != 0 {
            return Err(Errno::EIO);
        }
        let block_offset = usize::try_from(parent.size).map_err(|_| Errno::EFBIG)?;
        let mut block = Vec::new();
        block.resize(block_size, 0);
        let usable_len = self.init_dirent_tail(parent, &mut block)?;
        write_dirent(
            &mut block,
            0,
            child_ino,
            usable_len,
            name.as_bytes(),
            Self::dirent_type(child_type),
        )?;
        self.write_directory_block(parent, &mut block, block_offset)?;
        parent.set_size(parent.size.checked_add(self.info.block_size).ok_or(Errno::EFBIG)?);
        Ok(())
    }

    fn add_indexed_directory_entry(
        &mut self,
        parent: &mut Ext4InodeRaw,
        name: &str,
        child_ino: u32,
        child_type: FileType,
    ) -> SysResult<()> {
        let root_block = self.read_directory_logical_block(parent, 0)?;
        let (hash_version, _, _) = self.parse_htree_root(&root_block)?;
        let hash = htree_hash(name.as_bytes(), self.info.hash_seed, hash_version)?;
        let leaf = self.find_indexed_directory_leaf(parent, hash)?;
        let block_offset = directory_block_offset(self.info.block_size, leaf.leaf_logical_block)?;
        let mut block = self.read_directory_logical_block(parent, leaf.leaf_logical_block)?;
        let block_len = block.len();
        if self.try_insert_directory_entry(parent, &mut block, block_len, block_offset, name, child_ino, child_type)? {
            return Ok(());
        }

        self.split_indexed_directory_leaf(parent, leaf, hash_version, hash, name, child_ino, child_type)
    }

    fn try_insert_directory_entry(
        &mut self,
        parent: &mut Ext4InodeRaw,
        block: &mut [u8],
        block_len: usize,
        block_offset: usize,
        name: &str,
        child_ino: u32,
        child_type: FileType,
    ) -> SysResult<bool> {
        let needed = dirent_min_len(name.len());
        let mut offset = 0usize;
        while offset < block_len {
            let remaining = block_len - offset;
            if remaining < EXT4_DIR_ENTRY_HEADER_SIZE {
                return Err(Errno::EIO);
            }
            let entry_len = le_u16(block, offset + 4) as usize;
            let name_len = block[offset + 6] as usize;
            if entry_len == 0
                || entry_len < EXT4_DIR_ENTRY_HEADER_SIZE
                || entry_len > remaining
                || name_len > entry_len - EXT4_DIR_ENTRY_HEADER_SIZE
            {
                return Err(Errno::EIO);
            }

            let entry_ino = le_u32(block, offset);
            if entry_ino == 0 && entry_len >= needed {
                write_dirent(
                    block,
                    offset,
                    child_ino,
                    entry_len,
                    name.as_bytes(),
                    Self::dirent_type(child_type),
                )?;
                self.write_directory_block(parent, block, block_offset)?;
                return Ok(true);
            }

            let used_len = if entry_ino == 0 {
                EXT4_DIR_ENTRY_HEADER_SIZE
            } else {
                dirent_min_len(name_len)
            };
            if entry_len >= used_len.checked_add(needed).ok_or(Errno::EIO)? {
                write_le_u16(block, offset + 4, used_len as u16);
                write_dirent(
                    block,
                    offset + used_len,
                    child_ino,
                    entry_len - used_len,
                    name.as_bytes(),
                    Self::dirent_type(child_type),
                )?;
                self.write_directory_block(parent, block, block_offset)?;
                return Ok(true);
            }

            offset += entry_len;
        }

        Ok(false)
    }

    fn split_indexed_directory_leaf(
        &mut self,
        parent: &mut Ext4InodeRaw,
        mut leaf: HTreeLeafPath,
        hash_version: HTreeHashVersion,
        hash: u32,
        name: &str,
        child_ino: u32,
        child_type: FileType,
    ) -> SysResult<()> {
        if leaf.index_node.count >= leaf.index_node.limit {
            return Err(Errno::EOPNOTSUPP);
        }

        let old_block_offset = directory_block_offset(self.info.block_size, leaf.leaf_logical_block)?;
        let old_block = self.read_directory_logical_block(parent, leaf.leaf_logical_block)?;
        let mut entries = self.collect_hashed_directory_entries(&old_block, hash_version)?;
        entries.push(HashedDirectoryEntry {
            hash,
            ino: child_ino,
            name: Vec::from(name.as_bytes()),
            file_type: Self::dirent_type(child_type),
            rec_len: dirent_min_len(name.len()),
        });
        entries.sort_by_key(|entry| entry.hash);

        let usable_len = self.directory_data_usable_len(parent, old_block.len())?;
        let mid = split_hashed_entries(&entries, usable_len)?;
        let new_hash = entries[mid].hash + u32::from(entries[mid].hash == entries[mid - 1].hash);
        let new_logical_block = u32::try_from(parent.size / self.info.block_size).map_err(|_| Errno::EFBIG)?;
        let new_block_offset = usize::try_from(parent.size).map_err(|_| Errno::EFBIG)?;

        let mut left = Vec::new();
        left.resize(old_block.len(), 0);
        self.write_hashed_entries_to_block(parent, &mut left, &entries[..mid])?;
        let mut right = Vec::new();
        right.resize(old_block.len(), 0);
        self.write_hashed_entries_to_block(parent, &mut right, &entries[mid..])?;

        leaf.index_node = htree_insert_entry(
            &mut leaf.index_block,
            leaf.index_node,
            leaf.index_position,
            new_hash,
            new_logical_block,
        )?;
        self.set_htree_block_checksum(parent, &mut leaf.index_block, leaf.index_node)?;

        self.write_directory_block(parent, &mut right, new_block_offset)?;
        parent.set_size(parent.size.checked_add(self.info.block_size).ok_or(Errno::EFBIG)?);
        self.write_directory_block(parent, &mut left, old_block_offset)?;

        let written = self.write_inode_data(
            parent,
            &leaf.index_block,
            directory_block_offset(self.info.block_size, leaf.index_logical_block)?,
        )?;
        if written != leaf.index_block.len() {
            return Err(Errno::EIO);
        }
        Ok(())
    }

    fn collect_hashed_directory_entries(
        &self,
        block: &[u8],
        hash_version: HTreeHashVersion,
    ) -> SysResult<Vec<HashedDirectoryEntry>> {
        let mut entries = Vec::new();
        let mut offset = 0usize;
        while offset < block.len() {
            let remaining = block.len() - offset;
            if remaining < EXT4_DIR_ENTRY_HEADER_SIZE {
                return Err(Errno::EIO);
            }

            let ino = le_u32(block, offset);
            let entry_len = le_u16(block, offset + 4) as usize;
            let name_len = block[offset + 6] as usize;
            let file_type = block[offset + 7];
            if entry_len == 0
                || entry_len < EXT4_DIR_ENTRY_HEADER_SIZE
                || entry_len > remaining
                || name_len > entry_len - EXT4_DIR_ENTRY_HEADER_SIZE
                || name_len > EXT4_NAME_LEN
            {
                return Err(Errno::EIO);
            }

            if ino != 0 && name_len != 0 {
                let name_start = offset + EXT4_DIR_ENTRY_HEADER_SIZE;
                let name_end = name_start + name_len;
                let name = Vec::from(&block[name_start..name_end]);
                let hash = htree_hash(&name, self.info.hash_seed, hash_version)?;
                entries.push(HashedDirectoryEntry {
                    hash,
                    ino,
                    name,
                    file_type,
                    rec_len: dirent_min_len(name_len),
                });
            }

            offset += entry_len;
        }

        Ok(entries)
    }

    fn write_hashed_entries_to_block(
        &self,
        parent: &Ext4InodeRaw,
        block: &mut [u8],
        entries: &[HashedDirectoryEntry],
    ) -> SysResult<()> {
        let usable_len = self.init_dirent_tail(parent, block)?;
        let mut offset = 0usize;
        for (index, entry) in entries.iter().enumerate() {
            let rec_len = if index + 1 == entries.len() {
                usable_len.checked_sub(offset).ok_or(Errno::EIO)?
            } else {
                entry.rec_len
            };
            if rec_len < entry.rec_len {
                return Err(Errno::EIO);
            }
            write_dirent(block, offset, entry.ino, rec_len, &entry.name, entry.file_type)?;
            offset = offset.checked_add(entry.rec_len).ok_or(Errno::EIO)?;
        }
        Ok(())
    }

    fn directory_data_usable_len(&self, inode: &Ext4InodeRaw, block_len: usize) -> SysResult<usize> {
        let mut block = Vec::new();
        block.resize(block_len, 0);
        self.init_dirent_tail(inode, &mut block)
    }

    fn set_htree_block_checksum(&self, inode: &Ext4InodeRaw, block: &mut [u8], node: HTreeNode) -> SysResult<()> {
        if !self.metadata_csum_enabled() {
            return Ok(());
        }

        let tail = node
            .entry_offset
            .checked_add(node.limit as usize * EXT4_DX_ENTRY_SIZE)
            .ok_or(Errno::EIO)?;
        if tail.checked_add(8).ok_or(Errno::EIO)? > block.len() {
            return Err(Errno::EIO);
        }

        write_le_u32(block, tail + 4, 0);
        let checksum_len = node
            .entry_offset
            .checked_add(node.count as usize * EXT4_DX_ENTRY_SIZE)
            .ok_or(Errno::EIO)?;
        let mut checksum = self.metadata_csum_seed();
        checksum = crc32c(checksum, &inode.ino.to_le_bytes());
        checksum = crc32c(checksum, &inode.generation.to_le_bytes());
        checksum = crc32c(checksum, &block[..checksum_len]);
        checksum = crc32c(checksum, &block[tail..tail + 8]);
        write_le_u32(block, tail + 4, checksum);
        Ok(())
    }

    fn remove_directory_entry(
        &mut self,
        parent: &mut Ext4InodeRaw,
        name: &str,
        expected_type: Option<FileType>,
    ) -> SysResult<(u32, FileType)> {
        Self::validate_filename(name)?;
        self.ensure_inode_data_supported(parent)?;
        let mut location = self.find_directory_entry(parent, name)?.ok_or(Errno::ENOENT)?;
        if let Some(expected_type) = expected_type
            && location.file_type != expected_type
        {
            return match (expected_type, location.file_type) {
                (FileType::Directory, _) => Err(Errno::ENOTDIR),
                (_, FileType::Directory) => Err(Errno::EISDIR),
                _ => Err(Errno::EEXIST),
            };
        }

        if let Some(previous) = location.previous_entry_offset {
            let previous_len = le_u16(&location.block, previous + 4) as usize;
            let merged = previous_len.checked_add(location.entry_len).ok_or(Errno::EIO)?;
            write_le_u16(&mut location.block, previous + 4, merged as u16);
        } else {
            write_le_u32(&mut location.block, location.entry_offset, 0);
        }
        self.write_directory_block(parent, &mut location.block, location.block_offset)?;
        Ok((location.ino, location.file_type))
    }

    fn write_directory_block(
        &mut self,
        inode: &mut Ext4InodeRaw,
        block: &mut [u8],
        block_offset: usize,
    ) -> SysResult<()> {
        self.set_directory_block_checksum(inode, block)?;
        let written = self.write_inode_data(inode, block, block_offset)?;
        if written != block.len() {
            return Err(Errno::EIO);
        }
        Ok(())
    }

    fn init_dirent_tail(&self, inode: &Ext4InodeRaw, block: &mut [u8]) -> SysResult<usize> {
        if !self.metadata_csum_enabled() {
            return Ok(block.len());
        }
        if block.len() < EXT4_DIRENT_TAIL_SIZE {
            return Err(Errno::EIO);
        }
        let tail = block.len() - EXT4_DIRENT_TAIL_SIZE;
        write_dirent(block, tail, 0, EXT4_DIRENT_TAIL_SIZE, &[], EXT4_DIRENT_TAIL_FILE_TYPE)?;
        self.set_directory_block_checksum(inode, block)?;
        Ok(tail)
    }

    fn set_directory_block_checksum(&self, inode: &Ext4InodeRaw, block: &mut [u8]) -> SysResult<()> {
        if !self.metadata_csum_enabled() {
            return Ok(());
        }
        let Some(tail) = dirent_tail_offset(block) else {
            return Ok(());
        };
        write_le_u32(block, tail + 8, 0);
        let mut checksum = self.metadata_csum_seed();
        checksum = crc32c(checksum, &inode.ino.to_le_bytes());
        checksum = crc32c(checksum, &inode.generation.to_le_bytes());
        checksum = crc32c(checksum, block);
        write_le_u32(block, tail + 8, checksum);
        Ok(())
    }

    fn rename_child_same_parent(&mut self, parent_ino: u32, old_name: &str, new_name: &str) -> SysResult<()> {
        let mut parent = self.read_inode_raw(parent_ino)?;
        self.ensure_directory_mutable(&parent)?;
        let old_location = self.find_directory_entry(&parent, old_name)?.ok_or(Errno::ENOENT)?;
        let mut source = self.read_inode_raw(old_location.ino)?;
        let source_type = source.file_type();

        if let Some(target_location) = self.find_directory_entry(&parent, new_name)? {
            if target_location.ino == source.ino {
                self.remove_directory_entry(&mut parent, old_name, Some(source_type))?;
                let time = now();
                parent.set_mtime(&time);
                parent.set_ctime(&time);
                source.set_ctime(&time);
                self.mark_inode_dirty(parent_ino, parent)?;
                return self.mark_inode_dirty(source.ino, source);
            }

            let mut target = self.read_inode_raw(target_location.ino)?;
            let target_type = target.file_type();
            match (source_type, target_type) {
                (FileType::Directory, FileType::Directory) => {
                    if !self.is_empty_directory(&target)? {
                        return Err(Errno::ENOTEMPTY);
                    }
                }
                (FileType::Directory, _) => return Err(Errno::ENOTDIR),
                (_, FileType::Directory) => return Err(Errno::EISDIR),
                _ => {}
            }
            self.remove_directory_entry(&mut parent, new_name, Some(target_type))?;
            target.decrement_links_count()?;
            if target.links_count == 0 {
                self.free_inode(target.ino, target)?;
            } else {
                target.set_ctime(&now());
                self.mark_inode_dirty(target.ino, target)?;
            }
        }

        self.remove_directory_entry(&mut parent, old_name, Some(source_type))?;
        self.add_directory_entry(&mut parent, new_name, source.ino, source_type)?;
        let time = now();
        parent.set_mtime(&time);
        parent.set_ctime(&time);
        source.set_ctime(&time);
        self.mark_inode_dirty(parent_ino, parent)?;
        self.mark_inode_dirty(source.ino, source)
    }

    fn ensure_directory_mutable(&self, inode: &Ext4InodeRaw) -> SysResult<()> {
        if inode.file_type() != FileType::Directory {
            return Err(Errno::ENOTDIR);
        }
        if inode.flags & (EXT4_INODE_FLAG_IMMUTABLE | EXT4_INODE_FLAG_APPEND) != 0 {
            return Err(Errno::EPERM);
        }
        self.ensure_inode_data_supported(inode)
    }

    fn init_directory_inode(&mut self, inode: &mut Ext4InodeRaw, parent_ino: u32) -> SysResult<()> {
        let block_size = self.info.block_size as usize;
        let mut block = Vec::new();
        block.resize(block_size, 0);
        let usable_len = self.init_dirent_tail(inode, &mut block)?;
        let dot_len = dirent_min_len(1);
        if usable_len <= dot_len {
            return Err(Errno::EIO);
        }
        write_dirent(&mut block, 0, inode.ino, dot_len, b".", EXT4_DE_DIR)?;
        write_dirent(
            &mut block,
            dot_len,
            parent_ino,
            usable_len - dot_len,
            b"..",
            EXT4_DE_DIR,
        )?;
        self.write_directory_block(inode, &mut block, 0)?;
        inode.set_size(self.info.block_size);
        Ok(())
    }

    fn is_empty_directory(&mut self, inode: &Ext4InodeRaw) -> SysResult<bool> {
        if inode.file_type() != FileType::Directory {
            return Err(Errno::ENOTDIR);
        }
        for entry in self.directory_entries(inode, None)? {
            if entry.name != "." && entry.name != ".." {
                return Ok(false);
            }
        }
        Ok(true)
    }

    fn update_dotdot(&mut self, inode: &mut Ext4InodeRaw, parent_ino: u32) -> SysResult<()> {
        let mut location = self.find_directory_entry(inode, "..")?.ok_or(Errno::EIO)?;
        write_le_u32(&mut location.block, location.entry_offset, parent_ino);
        self.write_directory_block(inode, &mut location.block, location.block_offset)
    }

    fn free_inode_data(&mut self, inode: &mut Ext4InodeRaw) -> SysResult<()> {
        if inode.blocks_count == 0 {
            inode.block.fill(0);
            inode.raw[40..100].copy_from_slice(&inode.block);
            inode.set_size(0);
            return Ok(());
        }
        self.truncate_inode_data(inode, 0)
    }

    fn truncate_inode_data(&mut self, inode: &mut Ext4InodeRaw, new_size: u64) -> SysResult<()> {
        self.ensure_inode_data_supported(inode)?;
        let new_blocks = new_size.div_ceil(self.info.block_size);
        let root = parse_extent_header(&inode.block)?;
        if root.depth > 1 || root.entries > 1 && new_blocks == 0 {
            return Err(Errno::EOPNOTSUPP);
        }

        let mut leaf = self.append_leaf(inode)?;
        let mut freed_blocks = 0u64;
        while leaf.header.entries > 0 {
            let entry_offset = EXT4_EXTENT_HEADER_SIZE + (leaf.header.entries as usize - 1) * EXT4_EXTENT_ENTRY_SIZE;
            let extent = parse_extent(&leaf.node[entry_offset..entry_offset + EXT4_EXTENT_ENTRY_SIZE]);
            let extent_end = extent.first_block as u64 + extent.len as u64;
            if extent_end <= new_blocks {
                break;
            }
            if new_blocks <= extent.first_block as u64 {
                self.free_extent_range(extent.pblock, extent.len as u64)?;
                freed_blocks = freed_blocks.checked_add(extent.len as u64).ok_or(Errno::EIO)?;
                leaf.header.entries -= 1;
                write_extent_header_entries(&mut leaf.node, leaf.header.entries);
                leaf.node[entry_offset..entry_offset + EXT4_EXTENT_ENTRY_SIZE].fill(0);
            } else {
                let keep = u16::try_from(new_blocks - extent.first_block as u64).map_err(|_| Errno::EIO)?;
                let free_count = extent.len.checked_sub(keep).ok_or(Errno::EIO)? as u64;
                self.free_extent_range(extent.pblock + keep as u64, free_count)?;
                freed_blocks = freed_blocks.checked_add(free_count).ok_or(Errno::EIO)?;
                write_extent_len(&mut leaf.node, entry_offset, keep);
                break;
            }
        }

        if new_blocks == 0
            && let Some(block) = leaf.block
        {
            self.free_data_block(block)?;
            freed_blocks = freed_blocks.checked_add(1).ok_or(Errno::EIO)?;
            inode.set_extent_root();
        } else {
            self.write_append_leaf(inode, leaf)?;
        }

        inode.subtract_blocks(
            freed_blocks
                .checked_mul(self.info.block_size / EXT4_INODE_BLOCK_SIZE)
                .ok_or(Errno::EIO)?,
        )?;
        inode.set_size(new_size);
        Ok(())
    }

    fn free_extent_range(&mut self, first: u64, count: u64) -> SysResult<()> {
        for offset in 0..count {
            self.free_data_block(first.checked_add(offset).ok_or(Errno::EIO)?)?;
        }
        Ok(())
    }

    fn map_extent_block(&self, inode: &Ext4InodeRaw, logical_block: u32) -> SysResult<BlockMapping> {
        let mut node = inode.block.to_vec();
        let mut header = parse_extent_header(&node)?;
        let mut depth = header.depth;

        while depth > 0 {
            let index = self.find_extent_index(&node, header, logical_block)?;
            if index.leaf >= self.info.blocks_count {
                return Err(Errno::EIO);
            }
            node.resize(self.info.block_size as usize, 0);
            let offset = index.leaf.checked_mul(self.info.block_size).ok_or(Errno::EIO)?;
            self.read_at(offset, &mut node)?;
            header = parse_extent_header(&node)?;
            if header.depth + 1 != depth {
                return Err(Errno::EIO);
            }
            depth = header.depth;
        }

        let Some(extent) = self.find_extent(&node, header, logical_block)? else {
            return Ok(BlockMapping::Hole);
        };

        let end = extent.first_block.checked_add(extent.len as u32).ok_or(Errno::EIO)?;
        if logical_block < extent.first_block || logical_block >= end {
            return Ok(BlockMapping::Hole);
        }
        if extent.unwritten {
            return Ok(BlockMapping::Unwritten);
        }

        let pblock = extent
            .pblock
            .checked_add((logical_block - extent.first_block) as u64)
            .ok_or(Errno::EIO)?;
        if pblock >= self.info.blocks_count {
            return Err(Errno::EIO);
        }
        Ok(BlockMapping::Data(pblock))
    }

    fn find_extent_index(&self, node: &[u8], header: ExtentHeader, logical_block: u32) -> SysResult<ExtentIndex> {
        if header.entries == 0 {
            return Err(Errno::EIO);
        }

        let mut selected = None;
        for pos in 0..header.entries as usize {
            let offset = EXT4_EXTENT_HEADER_SIZE + pos * EXT4_EXTENT_ENTRY_SIZE;
            let index = parse_extent_index(&node[offset..offset + EXT4_EXTENT_ENTRY_SIZE]);
            if logical_block < index.first_block {
                break;
            }
            selected = Some(index);
        }
        selected.ok_or(Errno::EIO)
    }

    fn find_extent(&self, node: &[u8], header: ExtentHeader, logical_block: u32) -> SysResult<Option<Extent>> {
        if header.entries == 0 {
            return Ok(None);
        }

        let mut selected = None;
        for pos in 0..header.entries as usize {
            let offset = EXT4_EXTENT_HEADER_SIZE + pos * EXT4_EXTENT_ENTRY_SIZE;
            let extent = parse_extent(&node[offset..offset + EXT4_EXTENT_ENTRY_SIZE]);
            if logical_block < extent.first_block {
                break;
            }
            selected = Some(extent);
        }
        Ok(selected)
    }

    fn allocate_append_blocks(
        &mut self,
        inode: &mut Ext4InodeRaw,
        first_logical_block: u32,
        count: u32,
    ) -> SysResult<AllocatedBlockRun> {
        if count == 0 {
            return Err(Errno::EINVAL);
        }

        let mut remaining = count;
        let mut logical_block = first_logical_block;
        let mut first_run = None;
        while remaining > 0 {
            let goal = self.append_allocation_goal(inode, logical_block)?;
            let request = cmp::min(remaining, EXT4_EXTENT_MAX_INITIALIZED_LEN as u32);
            let run = self.allocate_data_block_run(goal, request)?;
            for offset in 0..run.len as u64 {
                if let Err(err) = self.zero_block(run.first_block.checked_add(offset).ok_or(Errno::EIO)?) {
                    let _ = self.free_extent_range(run.first_block, run.len as u64);
                    return Err(err);
                }
            }

            if let Err(err) = self.append_extent(
                inode,
                Extent {
                    first_block: logical_block,
                    len: run.len,
                    pblock: run.first_block,
                    unwritten: false,
                },
            ) {
                let _ = self.free_extent_range(run.first_block, run.len as u64);
                return Err(err);
            }

            inode.add_blocks(
                (run.len as u64)
                    .checked_mul(self.info.block_size / EXT4_INODE_BLOCK_SIZE)
                    .ok_or(Errno::EIO)?,
            )?;
            if first_run.is_none() {
                first_run = Some(run);
            }
            let run_len = run.len as u32;
            logical_block = logical_block.checked_add(run_len).ok_or(Errno::EFBIG)?;
            remaining = remaining.checked_sub(run_len).ok_or(Errno::EIO)?;
        }

        first_run.ok_or(Errno::EIO)
    }

    fn append_allocation_goal(&self, inode: &Ext4InodeRaw, logical_block: u32) -> SysResult<Option<u64>> {
        let leaf = self.append_leaf(inode)?;
        if leaf.header.entries == 0 {
            return Ok(None);
        }

        let last_offset = EXT4_EXTENT_HEADER_SIZE + (leaf.header.entries as usize - 1) * EXT4_EXTENT_ENTRY_SIZE;
        let last = parse_extent(&leaf.node[last_offset..last_offset + EXT4_EXTENT_ENTRY_SIZE]);
        let last_end = last.first_block.checked_add(last.len as u32).ok_or(Errno::EFBIG)?;
        if logical_block != last_end {
            if leaf.header.entries < leaf.header.max_entries {
                return Ok(None);
            }
            if leaf.block.is_none() && leaf.header.depth == 0 {
                return Ok(None);
            }
            return Err(Errno::EOPNOTSUPP);
        }

        if last.len < EXT4_EXTENT_MAX_INITIALIZED_LEN {
            return Ok(Some(last.pblock.checked_add(last.len as u64).ok_or(Errno::EIO)?));
        }
        if leaf.header.entries < leaf.header.max_entries {
            Ok(None)
        } else if leaf.block.is_none() && leaf.header.depth == 0 {
            Ok(None)
        } else {
            Err(Errno::EOPNOTSUPP)
        }
    }

    fn append_logical_end(&self, inode: &Ext4InodeRaw) -> SysResult<u32> {
        let leaf = self.append_leaf(inode)?;
        if leaf.header.entries == 0 {
            return Ok(0);
        }

        let last_offset = EXT4_EXTENT_HEADER_SIZE + (leaf.header.entries as usize - 1) * EXT4_EXTENT_ENTRY_SIZE;
        let last = parse_extent(&leaf.node[last_offset..last_offset + EXT4_EXTENT_ENTRY_SIZE]);
        last.first_block.checked_add(last.len as u32).ok_or(Errno::EFBIG)
    }

    fn append_extent(&mut self, inode: &mut Ext4InodeRaw, extent: Extent) -> SysResult<()> {
        let mut leaf = self.append_leaf(inode)?;
        if leaf.header.entries > 0 {
            let last_offset = EXT4_EXTENT_HEADER_SIZE + (leaf.header.entries as usize - 1) * EXT4_EXTENT_ENTRY_SIZE;
            let last = parse_extent(&leaf.node[last_offset..last_offset + EXT4_EXTENT_ENTRY_SIZE]);
            if Self::can_merge_extents(last, extent) {
                write_extent_len(&mut leaf.node, last_offset, last.len + extent.len);
                self.write_append_leaf(inode, leaf)?;
                return Ok(());
            }
        }

        if leaf.header.entries >= leaf.header.max_entries {
            if leaf.block.is_none() && leaf.header.depth == 0 {
                self.grow_inline_extent_root(inode, leaf)?;
                return self.append_extent(inode, extent);
            } else {
                return Err(Errno::EOPNOTSUPP);
            }
        }

        let insert_offset = EXT4_EXTENT_HEADER_SIZE + leaf.header.entries as usize * EXT4_EXTENT_ENTRY_SIZE;
        write_extent(&mut leaf.node, insert_offset, extent);
        leaf.header.entries += 1;
        write_extent_header_entries(&mut leaf.node, leaf.header.entries);
        self.write_append_leaf(inode, leaf)
    }

    fn grow_inline_extent_root(&mut self, inode: &mut Ext4InodeRaw, leaf: AppendLeaf) -> SysResult<()> {
        if leaf.block.is_some() || leaf.header.depth != 0 || leaf.header.entries == 0 {
            return Err(Errno::EOPNOTSUPP);
        }

        let block = self.allocate_data_block(None)?;
        let mut external_leaf = Vec::new();
        external_leaf.resize(self.info.block_size as usize, 0);
        let max_entries = self.extent_block_max_entries()?;
        write_extent_header(&mut external_leaf, leaf.header.entries, max_entries, 0);

        let entries_len = leaf.header.entries as usize * EXT4_EXTENT_ENTRY_SIZE;
        let entries_end = EXT4_EXTENT_HEADER_SIZE.checked_add(entries_len).ok_or(Errno::EIO)?;
        external_leaf[EXT4_EXTENT_HEADER_SIZE..entries_end]
            .copy_from_slice(&leaf.node[EXT4_EXTENT_HEADER_SIZE..entries_end]);
        self.set_extent_block_checksum(inode, &mut external_leaf)?;
        self.write_at(
            block.checked_mul(self.info.block_size).ok_or(Errno::EIO)?,
            &external_leaf,
        )?;

        let first_extent =
            parse_extent(&leaf.node[EXT4_EXTENT_HEADER_SIZE..EXT4_EXTENT_HEADER_SIZE + EXT4_EXTENT_ENTRY_SIZE]);
        inode.block.fill(0);
        write_extent_header(&mut inode.block, 1, self.inline_extent_index_max_entries()?, 1);
        write_extent_index(
            &mut inode.block,
            EXT4_EXTENT_HEADER_SIZE,
            first_extent.first_block,
            block,
        );
        inode.raw[40..100].copy_from_slice(&inode.block);
        inode.add_blocks(self.info.block_size / EXT4_INODE_BLOCK_SIZE)?;
        Ok(())
    }

    fn extent_block_max_entries(&self) -> SysResult<u16> {
        let tail_size = if self.metadata_csum_enabled() { 4 } else { 0 };
        let entries_size = self
            .info
            .block_size
            .checked_sub(EXT4_EXTENT_HEADER_SIZE as u64)
            .and_then(|size| size.checked_sub(tail_size))
            .ok_or(Errno::EIO)?;
        u16::try_from(entries_size / EXT4_EXTENT_ENTRY_SIZE as u64).map_err(|_| Errno::EIO)
    }

    fn inline_extent_index_max_entries(&self) -> SysResult<u16> {
        u16::try_from((60 - EXT4_EXTENT_HEADER_SIZE) / EXT4_EXTENT_ENTRY_SIZE).map_err(|_| Errno::EIO)
    }

    fn can_merge_extents(left: Extent, right: Extent) -> bool {
        !left.unwritten
            && !right.unwritten
            && left.first_block.checked_add(left.len as u32) == Some(right.first_block)
            && left.pblock.checked_add(left.len as u64) == Some(right.pblock)
            && matches!(
                left.len.checked_add(right.len),
                Some(len) if len <= EXT4_EXTENT_MAX_INITIALIZED_LEN
            )
    }

    fn append_leaf(&self, inode: &Ext4InodeRaw) -> SysResult<AppendLeaf> {
        let mut node = inode.block.to_vec();
        let mut header = parse_extent_header(&node)?;
        let mut depth = header.depth;
        let mut block = None;

        while depth > 0 {
            if header.entries == 0 {
                return Err(Errno::EIO);
            }
            let offset = EXT4_EXTENT_HEADER_SIZE + (header.entries as usize - 1) * EXT4_EXTENT_ENTRY_SIZE;
            let index = parse_extent_index(&node[offset..offset + EXT4_EXTENT_ENTRY_SIZE]);
            if index.leaf >= self.info.blocks_count {
                return Err(Errno::EIO);
            }

            node.resize(self.info.block_size as usize, 0);
            self.read_at(
                index.leaf.checked_mul(self.info.block_size).ok_or(Errno::EIO)?,
                &mut node,
            )?;
            block = Some(index.leaf);
            header = parse_extent_header(&node)?;
            if header.depth + 1 != depth {
                return Err(Errno::EIO);
            }
            depth = header.depth;
        }

        Ok(AppendLeaf { node, header, block })
    }

    fn write_append_leaf(&self, inode: &mut Ext4InodeRaw, mut leaf: AppendLeaf) -> SysResult<()> {
        if let Some(block) = leaf.block {
            self.set_extent_block_checksum(inode, &mut leaf.node)?;
            self.write_at(block.checked_mul(self.info.block_size).ok_or(Errno::EIO)?, &leaf.node)
        } else {
            inode.block.copy_from_slice(&leaf.node[..60]);
            inode.raw[40..100].copy_from_slice(&inode.block);
            Ok(())
        }
    }

    fn set_extent_block_checksum(&self, inode: &Ext4InodeRaw, node: &mut [u8]) -> SysResult<()> {
        if !self.metadata_csum_enabled() {
            return Ok(());
        }

        let header = parse_extent_header(node)?;
        let tail_offset = EXT4_EXTENT_HEADER_SIZE
            .checked_add(header.max_entries as usize * EXT4_EXTENT_ENTRY_SIZE)
            .ok_or(Errno::EIO)?;
        if tail_offset.checked_add(4).ok_or(Errno::EIO)? > node.len() {
            return Err(Errno::EIO);
        }

        let mut checksum = self.metadata_csum_seed();
        checksum = crc32c(checksum, &inode.ino.to_le_bytes());
        checksum = crc32c(checksum, &inode.generation.to_le_bytes());
        checksum = crc32c(checksum, &node[..tail_offset]);
        write_le_u32(node, tail_offset, checksum);
        Ok(())
    }

    fn allocate_data_block(&mut self, goal: Option<u64>) -> SysResult<u64> {
        Ok(self.allocate_data_block_run(goal, 1)?.first_block)
    }

    fn allocate_data_block_run(&mut self, goal: Option<u64>, count: u32) -> SysResult<AllocatedBlockRun> {
        if self.info.features_readonly & EXT4_FEATURE_RO_COMPAT_GDT_CSUM != 0 && !self.metadata_csum_enabled() {
            return Err(Errno::EOPNOTSUPP);
        }
        if count == 0 {
            return Err(Errno::EINVAL);
        }

        if let Some(goal) = goal {
            let (goal_group, goal_index) = self.block_to_group_index(goal)?;
            match self.try_allocate_block_run_in_group(goal_group, Some(goal_index), count) {
                Ok(run) => return Ok(run),
                Err(Errno::ENOSPC) => {}
                Err(err) => return Err(err),
            }

            for group_offset in 1..self.info.block_group_count() {
                let group = (goal_group + group_offset) % self.info.block_group_count();
                match self.try_allocate_block_run_in_group(group, None, count) {
                    Ok(run) => return Ok(run),
                    Err(Errno::ENOSPC) => {}
                    Err(err) => return Err(err),
                }
            }
            return Err(Errno::ENOSPC);
        }

        for group in 0..self.info.block_group_count() {
            match self.try_allocate_block_run_in_group(group, None, count) {
                Ok(run) => return Ok(run),
                Err(Errno::ENOSPC) => {}
                Err(err) => return Err(err),
            }
        }
        Err(Errno::ENOSPC)
    }

    fn try_allocate_block_run_in_group(
        &mut self,
        group: u32,
        preferred_index: Option<u32>,
        count: u32,
    ) -> SysResult<AllocatedBlockRun> {
        if count == 0 {
            return Err(Errno::EINVAL);
        }
        let mut descriptor = self.read_group_descriptor(group)?;
        if descriptor.free_blocks_count == 0 {
            return Err(Errno::ENOSPC);
        }

        let bitmap_offset = descriptor
            .block_bitmap_block
            .checked_mul(self.info.block_size)
            .ok_or(Errno::EIO)?;
        let mut bitmap = Vec::new();
        bitmap.resize(self.info.block_size as usize, 0);
        self.read_at(bitmap_offset, &mut bitmap)?;

        let mut selected = None;
        let start = preferred_index.unwrap_or(0).min(self.info.blocks_per_group);
        for index in start..self.info.blocks_per_group {
            let block = self.group_index_to_block(group, index)?;
            if block >= self.info.blocks_count {
                break;
            }
            if bitmap_get(&bitmap, index) || block >= self.info.blocks_count {
                continue;
            }
            selected = Some(index);
            break;
        }
        if selected.is_none() && start != 0 {
            for index in 0..start {
                let block = self.group_index_to_block(group, index)?;
                if block >= self.info.blocks_count {
                    break;
                }
                if !bitmap_get(&bitmap, index) {
                    selected = Some(index);
                    break;
                }
            }
        }
        let selected = selected.ok_or(Errno::ENOSPC)?;

        let mut len = 0u32;
        let max_len = cmp::min(count, descriptor.free_blocks_count as u32);
        while len < max_len {
            let index = selected.checked_add(len).ok_or(Errno::EIO)?;
            if index >= self.info.blocks_per_group {
                break;
            }
            let block = self.group_index_to_block(group, index)?;
            if block >= self.info.blocks_count || bitmap_get(&bitmap, index) {
                break;
            }
            len += 1;
        }
        if len == 0 {
            return Err(Errno::ENOSPC);
        }

        for index in selected..selected + len {
            bitmap_set(&mut bitmap, index);
        }
        let first_block = self.group_index_to_block(group, selected)?;
        self.update_block_bitmap_checksum(&mut descriptor, &bitmap)?;
        self.write_at(bitmap_offset, &bitmap)?;
        descriptor.free_blocks_count = descriptor.free_blocks_count.checked_sub(len as u64).ok_or(Errno::EIO)?;
        descriptor.set_free_blocks_count(self.info.desc_size, descriptor.free_blocks_count);
        self.write_group_descriptor(group, &mut descriptor)?;

        self.info.free_blocks_count = self.info.free_blocks_count.checked_sub(len as u64).ok_or(Errno::EIO)?;
        self.set_superblock_free_blocks_count(self.info.free_blocks_count)?;
        Ok(AllocatedBlockRun {
            first_block,
            len: u16::try_from(len).map_err(|_| Errno::EIO)?,
        })
    }

    fn block_to_group_index(&self, block: u64) -> SysResult<(u32, u32)> {
        if block >= self.info.blocks_count {
            return Err(Errno::EIO);
        }
        let block = if self.info.first_data_block != 0 {
            block.checked_sub(1).ok_or(Errno::EIO)?
        } else {
            block
        };
        let group = block / self.info.blocks_per_group as u64;
        let index = block % self.info.blocks_per_group as u64;
        Ok((
            u32::try_from(group).map_err(|_| Errno::EIO)?,
            u32::try_from(index).map_err(|_| Errno::EIO)?,
        ))
    }

    fn group_index_to_block(&self, group: u32, index: u32) -> SysResult<u64> {
        let base = (group as u64)
            .checked_mul(self.info.blocks_per_group as u64)
            .and_then(|block| block.checked_add(index as u64))
            .ok_or(Errno::EIO)?;
        if self.info.first_data_block != 0 {
            base.checked_add(1).ok_or(Errno::EIO)
        } else {
            Ok(base)
        }
    }

    fn zero_block(&self, block: u64) -> SysResult<()> {
        let mut zero = Vec::new();
        zero.resize(self.info.block_size as usize, 0);
        self.write_at(block.checked_mul(self.info.block_size).ok_or(Errno::EIO)?, &zero)
    }

    fn allocate_inode(&mut self, mode: Mode, owner: Owner, links: u16) -> SysResult<Ext4InodeRaw> {
        if self.info.features_readonly & EXT4_FEATURE_RO_COMPAT_GDT_CSUM != 0 && !self.metadata_csum_enabled() {
            return Err(Errno::EOPNOTSUPP);
        }

        let file_type: FileType = mode.into();
        for group in 0..self.info.block_group_count() {
            let mut descriptor = self.read_group_descriptor(group)?;
            if descriptor.free_inodes_count == 0 {
                continue;
            }

            let bitmap_offset = descriptor
                .inode_bitmap_block
                .checked_mul(self.info.block_size)
                .ok_or(Errno::EIO)?;
            let mut bitmap = Vec::new();
            bitmap.resize(self.info.block_size as usize, 0);
            self.read_at(bitmap_offset, &mut bitmap)?;

            for index in 0..self.info.inodes_per_group {
                let ino = group
                    .checked_mul(self.info.inodes_per_group)
                    .and_then(|base| base.checked_add(index))
                    .and_then(|value| value.checked_add(1))
                    .ok_or(Errno::EIO)?;
                if ino > self.info.inodes_count {
                    break;
                }
                if bitmap_get(&bitmap, index) {
                    continue;
                }

                bitmap_set(&mut bitmap, index);
                self.update_inode_bitmap_checksum(&mut descriptor, &bitmap)?;
                self.write_at(bitmap_offset, &bitmap)?;

                descriptor.free_inodes_count = descriptor.free_inodes_count.checked_sub(1).ok_or(Errno::EIO)?;
                descriptor.set_free_inodes_count(self.info.desc_size, descriptor.free_inodes_count);
                if file_type == FileType::Directory {
                    descriptor.used_dirs_count = descriptor.used_dirs_count.checked_add(1).ok_or(Errno::EIO)?;
                    descriptor.set_used_dirs_count(self.info.desc_size, descriptor.used_dirs_count);
                }
                self.write_group_descriptor(group, &mut descriptor)?;

                self.info.free_inodes_count = self.info.free_inodes_count.checked_sub(1).ok_or(Errno::EIO)?;
                self.set_superblock_free_inodes_count(self.info.free_inodes_count)?;

                let inode = self.new_inode_raw(ino, mode, owner, links)?;
                self.mark_inode_dirty(ino, inode.clone())?;
                return Ok(inode);
            }
        }

        Err(Errno::ENOSPC)
    }

    fn free_inode(&mut self, ino: u32, mut inode: Ext4InodeRaw) -> SysResult<()> {
        self.free_inode_data(&mut inode)?;

        let (group, index) = self.inode_group_index(ino)?;
        let mut descriptor = self.read_group_descriptor(group)?;
        let bitmap_offset = descriptor
            .inode_bitmap_block
            .checked_mul(self.info.block_size)
            .ok_or(Errno::EIO)?;
        let mut bitmap = Vec::new();
        bitmap.resize(self.info.block_size as usize, 0);
        self.read_at(bitmap_offset, &mut bitmap)?;
        if bitmap_get(&bitmap, index) {
            bitmap_clear(&mut bitmap, index);
            self.update_inode_bitmap_checksum(&mut descriptor, &bitmap)?;
            self.write_at(bitmap_offset, &bitmap)?;

            descriptor.free_inodes_count = descriptor.free_inodes_count.checked_add(1).ok_or(Errno::EIO)?;
            descriptor.set_free_inodes_count(self.info.desc_size, descriptor.free_inodes_count);
            if inode.file_type() == FileType::Directory {
                descriptor.used_dirs_count = descriptor.used_dirs_count.checked_sub(1).ok_or(Errno::EIO)?;
                descriptor.set_used_dirs_count(self.info.desc_size, descriptor.used_dirs_count);
            }
            self.write_group_descriptor(group, &mut descriptor)?;

            self.info.free_inodes_count = self.info.free_inodes_count.checked_add(1).ok_or(Errno::EIO)?;
            self.set_superblock_free_inodes_count(self.info.free_inodes_count)?;
        }

        let mut raw = Vec::new();
        raw.resize(self.info.inode_size as usize, 0);
        let inode_offset = self.inode_offset(ino)?;
        self.write_at(inode_offset, &raw)?;
        self.inode_cache.remove(&ino);
        Ok(())
    }

    fn new_inode_raw(&self, ino: u32, mode: Mode, owner: Owner, links: u16) -> SysResult<Ext4InodeRaw> {
        let mut raw = Vec::new();
        raw.resize(self.info.inode_size as usize, 0);
        if raw.len() > 130 {
            write_le_u16(&mut raw, 128, 32);
        }
        let mut inode = Ext4InodeRaw::parse(&raw)?;
        inode.ino = ino;
        inode.set_mode(mode.bits() as u16);
        inode.set_owner(owner.uid, owner.gid);
        inode.set_links_count(links);
        inode.set_generation(ino ^ now().as_secs() as u32);
        let time = now();
        inode.set_atime(&time);
        inode.set_mtime(&time);
        inode.set_ctime(&time);
        if matches!(
            inode.file_type(),
            FileType::Regular | FileType::Directory | FileType::Symlink
        ) {
            inode.set_extent_root();
        }
        Ok(inode)
    }

    fn free_data_block(&mut self, block: u64) -> SysResult<()> {
        let (group, index) = self.block_to_group_index(block)?;
        let mut descriptor = self.read_group_descriptor(group)?;
        let bitmap_offset = descriptor
            .block_bitmap_block
            .checked_mul(self.info.block_size)
            .ok_or(Errno::EIO)?;
        let mut bitmap = Vec::new();
        bitmap.resize(self.info.block_size as usize, 0);
        self.read_at(bitmap_offset, &mut bitmap)?;
        if !bitmap_get(&bitmap, index) {
            return Ok(());
        }

        bitmap_clear(&mut bitmap, index);
        self.update_block_bitmap_checksum(&mut descriptor, &bitmap)?;
        self.write_at(bitmap_offset, &bitmap)?;
        descriptor.free_blocks_count = descriptor.free_blocks_count.checked_add(1).ok_or(Errno::EIO)?;
        descriptor.set_free_blocks_count(self.info.desc_size, descriptor.free_blocks_count);
        self.write_group_descriptor(group, &mut descriptor)?;

        self.info.free_blocks_count = self.info.free_blocks_count.checked_add(1).ok_or(Errno::EIO)?;
        self.set_superblock_free_blocks_count(self.info.free_blocks_count)?;
        Ok(())
    }

    fn update_block_bitmap_checksum(&self, descriptor: &mut GroupDescriptor, bitmap: &[u8]) -> SysResult<()> {
        if !self.metadata_csum_enabled() {
            return Ok(());
        }

        let bitmap_len = cmp::min(bitmap.len(), self.info.blocks_per_group.div_ceil(8) as usize);
        let checksum = crc32c(self.metadata_csum_seed(), &bitmap[..bitmap_len]);
        write_le_u16(&mut descriptor.raw, 24, checksum as u16);
        if self.info.desc_size as usize >= 64 {
            write_le_u16(&mut descriptor.raw, 56, (checksum >> 16) as u16);
        }
        Ok(())
    }

    fn update_inode_bitmap_checksum(&self, descriptor: &mut GroupDescriptor, bitmap: &[u8]) -> SysResult<()> {
        if !self.metadata_csum_enabled() {
            return Ok(());
        }

        let bitmap_len = cmp::min(bitmap.len(), self.info.inodes_per_group.div_ceil(8) as usize);
        let checksum = crc32c(self.metadata_csum_seed(), &bitmap[..bitmap_len]);
        write_le_u16(&mut descriptor.raw, 26, checksum as u16);
        if self.info.desc_size as usize >= 64 {
            write_le_u16(&mut descriptor.raw, 58, (checksum >> 16) as u16);
        }
        Ok(())
    }

    fn write_group_descriptor(&mut self, group: u32, descriptor: &mut GroupDescriptor) -> SysResult<()> {
        if self.metadata_csum_enabled() {
            write_le_u16(&mut descriptor.raw, 30, 0);
            let mut checksum = self.metadata_csum_seed();
            checksum = crc32c(checksum, &group.to_le_bytes());
            checksum = crc32c(checksum, &descriptor.raw[..self.info.desc_size as usize]);
            write_le_u16(&mut descriptor.raw, 30, checksum as u16);
        }

        let offset = self.group_descriptor_offset(group)?;
        self.write_at(offset, &descriptor.raw)?;
        self.insert_group_descriptor_cache(group, descriptor.clone());
        Ok(())
    }

    fn set_superblock_free_blocks_count(&mut self, free_blocks: u64) -> SysResult<()> {
        write_le_u32(&mut self.superblock_raw, 12, free_blocks as u32);
        write_le_u32(&mut self.superblock_raw, 344, (free_blocks >> 32) as u32);
        self.write_superblock()
    }

    fn set_superblock_free_inodes_count(&mut self, free_inodes: u32) -> SysResult<()> {
        write_le_u32(&mut self.superblock_raw, 16, free_inodes);
        self.write_superblock()
    }

    fn write_superblock(&mut self) -> SysResult<()> {
        if self.metadata_csum_enabled() {
            if self.info.checksum_type != EXT4_CHECKSUM_CRC32C {
                return Err(Errno::EOPNOTSUPP);
            }
            write_le_u32(&mut self.superblock_raw, 1020, 0);
            let checksum = crc32c(EXT4_CRC32_INIT, &self.superblock_raw[..1020]);
            write_le_u32(&mut self.superblock_raw, 1020, checksum);
        }
        self.write_at(SUPERBLOCK_OFFSET, &self.superblock_raw)
    }

    fn mark_inode_dirty(&mut self, ino: u32, inode: Ext4InodeRaw) -> SysResult<()> {
        self.insert_inode_cache(ino, inode, true)
    }

    fn flush_dirty_inode_metadata(&mut self) -> SysResult<()> {
        let mut dirty_inodes = Vec::new();
        self.inode_cache.try_for_each_mut::<Errno, _>(|ino, cached| {
            if cached.dirty {
                dirty_inodes.push(ino);
            }
            Ok(())
        })?;

        for ino in dirty_inodes {
            self.flush_inode_metadata(ino)?;
        }
        Ok(())
    }

    fn flush_inode_metadata(&mut self, ino: u32) -> SysResult<()> {
        let mut inode = {
            let Some(cached) = self.inode_cache.get(&ino) else {
                return Ok(());
            };
            if !cached.dirty {
                return Ok(());
            }
            cached.inode.clone()
        };

        self.write_inode_raw_to_disk(ino, &mut inode)?;
        if let Some(cached) = self.inode_cache.get_mut(&ino) {
            cached.inode = inode;
            cached.dirty = false;
        }
        Ok(())
    }

    fn write_inode_raw_to_disk(&mut self, ino: u32, inode: &mut Ext4InodeRaw) -> SysResult<()> {
        if self.metadata_csum_enabled() {
            inode.set_checksum(0, self.info.inode_size);
            let mut checksum = self.metadata_csum_seed();
            checksum = crc32c(checksum, &ino.to_le_bytes());
            checksum = crc32c(checksum, &inode.generation.to_le_bytes());
            checksum = crc32c(checksum, &inode.raw[..self.info.inode_size as usize]);
            if self.info.inode_size == EXT4_MIN_INODE_SIZE {
                checksum &= 0xffff;
            }
            inode.set_checksum(checksum, self.info.inode_size);
        }

        let offset = self.inode_offset(ino)?;
        self.write_at(offset, &inode.raw)?;
        Ok(())
    }

    fn inode_offset(&mut self, ino: u32) -> SysResult<u64> {
        if !(1..=self.info.inodes_count).contains(&ino) {
            return Err(Errno::ENOENT);
        }
        let (group, index) = self.inode_group_index(ino)?;
        let descriptor = self.read_group_descriptor(group)?;
        descriptor
            .inode_table_block
            .checked_mul(self.info.block_size)
            .and_then(|offset| offset.checked_add(index as u64 * self.info.inode_size as u64))
            .ok_or(Errno::EIO)
    }

    fn inode_group_index(&self, ino: u32) -> SysResult<(u32, u32)> {
        if !(1..=self.info.inodes_count).contains(&ino) {
            return Err(Errno::ENOENT);
        }
        let inodes_per_group = self.info.inodes_per_group;
        if inodes_per_group == 0 {
            return Err(Errno::EINVAL);
        }
        let group = (ino - 1) / inodes_per_group;
        let index = (ino - 1) % inodes_per_group;
        Ok((group, index))
    }

    fn group_descriptor_offset(&self, group: u32) -> SysResult<u64> {
        (self.info.first_data_block as u64 + 1)
            .checked_mul(self.info.block_size)
            .and_then(|base| base.checked_add(group as u64 * self.info.desc_size as u64))
            .ok_or(Errno::EIO)
    }

    fn metadata_csum_enabled(&self) -> bool {
        self.info.features_readonly & EXT4_FEATURE_RO_COMPAT_METADATA_CSUM != 0
    }

    fn metadata_csum_seed(&self) -> u32 {
        self.info.checksum_seed
    }

    fn read_inode_raw(&mut self, ino: u32) -> SysResult<Ext4InodeRaw> {
        if !(1..=self.info.inodes_count).contains(&ino) {
            return Err(Errno::ENOENT);
        }
        if let Some(inode) = self.cached_inode_raw(ino) {
            return Ok(inode);
        }

        let inodes_per_group = self.info.inodes_per_group;
        if inodes_per_group == 0 {
            return Err(Errno::EINVAL);
        }
        let group = (ino - 1) / inodes_per_group;
        let index = (ino - 1) % inodes_per_group;
        let group_count = self.info.block_group_count();
        if group >= group_count {
            return Err(Errno::ENOENT);
        }

        let descriptor = self.read_group_descriptor(group)?;
        if descriptor.inode_table_block >= self.info.blocks_count {
            return Err(Errno::EIO);
        }

        let inode_offset = descriptor
            .inode_table_block
            .checked_mul(self.info.block_size)
            .and_then(|offset| offset.checked_add(index as u64 * self.info.inode_size as u64))
            .ok_or(Errno::EIO)?;

        let mut raw = Vec::new();
        raw.resize(self.info.inode_size as usize, 0);
        self.read_at(inode_offset, &mut raw)?;
        let mut inode = Ext4InodeRaw::parse(&raw)?;
        inode.ino = ino;
        if inode.mode == 0 || inode.links_count == 0 {
            return Err(Errno::ENOENT);
        }
        self.insert_inode_cache(ino, inode.clone(), false)?;
        Ok(inode)
    }

    fn read_group_descriptor(&mut self, group: u32) -> SysResult<GroupDescriptor> {
        if let Some(descriptor) = self.cached_group_descriptor(group) {
            return Ok(descriptor);
        }

        let offset = self.group_descriptor_offset(group)?;
        let mut raw = Vec::new();
        raw.resize(self.info.desc_size as usize, 0);
        self.read_at(offset, &mut raw)?;
        let descriptor = GroupDescriptor::parse(&raw, self.info.desc_size);
        self.insert_group_descriptor_cache(group, descriptor.clone());
        Ok(descriptor)
    }

    fn read_at(&self, offset: u64, buf: &mut [u8]) -> SysResult<()> {
        let offset = usize::try_from(offset).map_err(|_| Errno::EIO)?;
        self.driver.read_at(offset, buf).map_err(|_| Errno::EIO)
    }

    fn write_at(&self, offset: u64, buf: &[u8]) -> SysResult<()> {
        if self.is_readonly() {
            return Err(Errno::EROFS);
        }
        let offset = usize::try_from(offset).map_err(|_| Errno::EIO)?;
        self.driver.write_at(offset, buf).map_err(|_| Errno::EIO)
    }

    fn cached_inode_raw(&mut self, ino: u32) -> Option<Ext4InodeRaw> {
        self.inode_cache.get(&ino).map(|cached| cached.inode.clone())
    }

    fn insert_inode_cache(&mut self, ino: u32, inode: Ext4InodeRaw, dirty: bool) -> SysResult<()> {
        if let Some(cached) = self.inode_cache.get_mut(&ino) {
            if dirty || !cached.dirty {
                cached.inode = inode;
            }
            cached.dirty |= dirty;
            return Ok(());
        }
        if INODE_METADATA_CACHE_SIZE == 0 {
            if dirty {
                let mut inode = inode;
                self.write_inode_raw_to_disk(ino, &mut inode)?;
            }
            return Ok(());
        }

        while self.inode_cache.len() >= INODE_METADATA_CACHE_SIZE {
            let Some((evicted_ino, mut cached)) = self.inode_cache.pop_lru_entry() else {
                break;
            };
            if let Err(err) = self.flush_cached_inode(evicted_ino, &mut cached) {
                self.inode_cache.put(evicted_ino, cached);
                return Err(err);
            }
        }

        self.inode_cache.put(ino, CachedInodeRaw { inode, dirty });
        Ok(())
    }

    fn flush_cached_inode(&mut self, ino: u32, cached: &mut CachedInodeRaw) -> SysResult<()> {
        if cached.dirty {
            self.write_inode_raw_to_disk(ino, &mut cached.inode)?;
            cached.dirty = false;
        }
        Ok(())
    }

    fn cached_group_descriptor(&mut self, group: u32) -> Option<GroupDescriptor> {
        self.group_desc_cache.get(&group).cloned()
    }

    fn insert_group_descriptor_cache(&mut self, group: u32, descriptor: GroupDescriptor) {
        insert_lru_cache(
            &mut self.group_desc_cache,
            GROUP_DESCRIPTOR_CACHE_SIZE,
            group,
            descriptor,
        );
    }
}

impl Ext4Info {
    fn read_from(driver: Arc<dyn BlockDriverOps>) -> SysResult<(Self, [u8; SUPERBLOCK_SIZE])> {
        let mut raw = [0u8; SUPERBLOCK_SIZE];
        driver
            .read_at(SUPERBLOCK_OFFSET as usize, &mut raw)
            .map_err(|_| Errno::EIO)?;

        if le_u16(&raw, 56) != EXT4_SUPER_MAGIC {
            return Err(Errno::EINVAL);
        }

        let log_block_size = le_u32(&raw, 24);
        if log_block_size > 2 {
            return Err(Errno::EOPNOTSUPP);
        }
        let block_size = 1024u64.checked_shl(log_block_size).ok_or(Errno::EINVAL)?;
        if !(1024..=EXT4_MAX_BLOCK_SIZE).contains(&block_size) {
            return Err(Errno::EINVAL);
        }

        let blocks_count = le_u32(&raw, 4) as u64 | ((le_u32(&raw, 336) as u64) << 32);
        let free_blocks_count = le_u32(&raw, 12) as u64 | ((le_u32(&raw, 344) as u64) << 32);
        let inodes_count = le_u32(&raw, 0);
        let free_inodes_count = le_u32(&raw, 16);
        let first_data_block = le_u32(&raw, 20);
        let blocks_per_group = le_u32(&raw, 32);
        let inodes_per_group = le_u32(&raw, 40);
        let features_incompatible = Ext4IncompatFeatures::from_bits_retain(le_u32(&raw, 96));
        let features_readonly = le_u32(&raw, 100);
        let mut uuid = [0u8; 16];
        uuid.copy_from_slice(&raw[104..120]);
        let hash_seed = [
            le_u32(&raw, 236),
            le_u32(&raw, 240),
            le_u32(&raw, 244),
            le_u32(&raw, 248),
        ];
        let flags = le_u32(&raw, 372);
        let checksum_seed = if features_incompatible.contains(Ext4IncompatFeatures::CSUM_SEED) {
            le_u32(&raw, 0x270)
        } else {
            crc32c(EXT4_CRC32_INIT, &uuid)
        };
        let checksum_type = raw[373];
        let inode_size = match le_u32(&raw, 76) {
            0 => EXT4_MIN_INODE_SIZE,
            _ => le_u16(&raw, 88),
        };
        let mut desc_size = if features_incompatible.contains(Ext4IncompatFeatures::BIT64) {
            le_u16(&raw, 254)
        } else {
            EXT4_MIN_DESC_SIZE
        };
        if desc_size < EXT4_MIN_DESC_SIZE {
            desc_size = EXT4_MIN_DESC_SIZE;
        }

        if blocks_count == 0 || inodes_count == 0 || blocks_per_group == 0 || inodes_per_group == 0 {
            return Err(Errno::EINVAL);
        }
        if inode_size < EXT4_MIN_INODE_SIZE || inode_size as u64 > block_size {
            return Err(Errno::EINVAL);
        }
        if desc_size < EXT4_MIN_DESC_SIZE || desc_size as u64 > block_size {
            return Err(Errno::EINVAL);
        }

        let unsupported_incompat = features_incompatible.bits() & !Ext4IncompatFeatures::supported().bits();
        if unsupported_incompat != 0 {
            return Err(Errno::EOPNOTSUPP);
        }
        if features_readonly & EXT4_FEATURE_RO_COMPAT_BIGALLOC != 0 {
            return Err(Errno::EOPNOTSUPP);
        }

        let device_bytes = (driver.get_block_size() as u64)
            .checked_mul(driver.get_block_count())
            .ok_or(Errno::EINVAL)?;
        let fs_bytes = blocks_count.checked_mul(block_size).ok_or(Errno::EINVAL)?;
        if fs_bytes > device_bytes {
            return Err(Errno::EINVAL);
        }

        Ok((
            Self {
                block_size,
                blocks_count,
                free_blocks_count,
                inodes_count,
                free_inodes_count,
                first_data_block,
                blocks_per_group,
                inodes_per_group,
                inode_size,
                desc_size,
                features_readonly,
                features_incompatible,
                hash_seed,
                flags,
                checksum_seed,
                checksum_type,
            },
            raw,
        ))
    }

    fn block_group_count(&self) -> u32 {
        self.blocks_count.div_ceil(self.blocks_per_group as u64) as u32
    }
}

impl GroupDescriptor {
    fn parse(raw: &[u8], desc_size: u16) -> Self {
        let block_bitmap_lo = le_u32(raw, 0) as u64;
        let inode_bitmap_lo = le_u32(raw, 4) as u64;
        let inode_table_lo = le_u32(raw, 8) as u64;
        let free_blocks_lo = le_u16(raw, 12) as u64;
        let free_inodes_lo = le_u16(raw, 14) as u64;
        let used_dirs_lo = le_u16(raw, 16) as u64;
        let block_bitmap_hi = if desc_size > EXT4_MIN_DESC_SIZE {
            le_u32(raw, 32) as u64
        } else {
            0
        };
        let inode_bitmap_hi = if desc_size > EXT4_MIN_DESC_SIZE {
            le_u32(raw, 36) as u64
        } else {
            0
        };
        let inode_table_hi = if desc_size > EXT4_MIN_DESC_SIZE {
            le_u32(raw, 40) as u64
        } else {
            0
        };
        let free_blocks_hi = if desc_size > EXT4_MIN_DESC_SIZE {
            le_u16(raw, 44) as u64
        } else {
            0
        };
        let free_inodes_hi = if desc_size > EXT4_MIN_DESC_SIZE {
            le_u16(raw, 46) as u64
        } else {
            0
        };
        let used_dirs_hi = if desc_size > EXT4_MIN_DESC_SIZE {
            le_u16(raw, 48) as u64
        } else {
            0
        };
        Self {
            block_bitmap_block: block_bitmap_lo | (block_bitmap_hi << 32),
            inode_bitmap_block: inode_bitmap_lo | (inode_bitmap_hi << 32),
            inode_table_block: inode_table_lo | (inode_table_hi << 32),
            free_blocks_count: free_blocks_lo | (free_blocks_hi << 16),
            free_inodes_count: free_inodes_lo | (free_inodes_hi << 16),
            used_dirs_count: used_dirs_lo | (used_dirs_hi << 16),
            raw: raw.to_vec(),
        }
    }

    fn set_free_blocks_count(&mut self, desc_size: u16, count: u64) {
        write_le_u16(&mut self.raw, 12, count as u16);
        if desc_size > EXT4_MIN_DESC_SIZE {
            write_le_u16(&mut self.raw, 44, (count >> 16) as u16);
        }
    }

    fn set_free_inodes_count(&mut self, desc_size: u16, count: u64) {
        write_le_u16(&mut self.raw, 14, count as u16);
        if desc_size > EXT4_MIN_DESC_SIZE {
            write_le_u16(&mut self.raw, 46, (count >> 16) as u16);
        }
    }

    fn set_used_dirs_count(&mut self, desc_size: u16, count: u64) {
        write_le_u16(&mut self.raw, 16, count as u16);
        if desc_size > EXT4_MIN_DESC_SIZE {
            write_le_u16(&mut self.raw, 48, (count >> 16) as u16);
        }
    }
}

impl Ext4InodeRaw {
    fn parse(raw: &[u8]) -> SysResult<Self> {
        if raw.len() < EXT4_MIN_INODE_SIZE as usize {
            return Err(Errno::EIO);
        }

        let mode = le_u16(raw, 0);
        let uid = le_u16(raw, 2) as u32 | ((le_u16(raw, 120) as u32) << 16);
        let gid = le_u16(raw, 24) as u32 | ((le_u16(raw, 122) as u32) << 16);
        let size_lo = le_u32(raw, 4) as u64;
        let size_hi = le_u32(raw, 108) as u64;
        let blocks_count = le_u32(raw, 28) as u64 | ((le_u16(raw, 116) as u64) << 32);
        let mut block = [0u8; 60];
        block.copy_from_slice(&raw[40..100]);

        Ok(Self {
            ino: 0,
            mode,
            uid,
            gid,
            size: size_lo | (size_hi << 32),
            atime: le_u32(raw, 8),
            ctime: le_u32(raw, 12),
            mtime: le_u32(raw, 16),
            generation: le_u32(raw, 100),
            links_count: le_u16(raw, 26),
            blocks_count,
            flags: le_u32(raw, 32),
            block,
            raw: raw.to_vec(),
        })
    }

    fn file_type(&self) -> FileType {
        Mode::from_bits_truncate(self.mode as u32).into()
    }

    fn set_size(&mut self, size: u64) {
        self.size = size;
        write_le_u32(&mut self.raw, 4, size as u32);
        write_le_u32(&mut self.raw, 108, (size >> 32) as u32);
    }

    fn set_mode(&mut self, mode: u16) {
        self.mode = mode;
        write_le_u16(&mut self.raw, 0, mode);
    }

    fn set_owner(&mut self, uid: u32, gid: u32) {
        self.uid = uid;
        self.gid = gid;
        write_le_u16(&mut self.raw, 2, uid as u16);
        write_le_u16(&mut self.raw, 120, (uid >> 16) as u16);
        write_le_u16(&mut self.raw, 24, gid as u16);
        write_le_u16(&mut self.raw, 122, (gid >> 16) as u16);
    }

    fn set_links_count(&mut self, links: u16) {
        self.links_count = links;
        write_le_u16(&mut self.raw, 26, links);
    }

    fn increment_links_count(&mut self) -> SysResult<()> {
        self.set_links_count(self.links_count.checked_add(1).ok_or(Errno::EFBIG)?);
        Ok(())
    }

    fn decrement_links_count(&mut self) -> SysResult<()> {
        self.set_links_count(self.links_count.checked_sub(1).ok_or(Errno::EIO)?);
        Ok(())
    }

    fn set_flags(&mut self, flags: u32) {
        self.flags = flags;
        write_le_u32(&mut self.raw, 32, flags);
    }

    fn set_generation(&mut self, generation: u32) {
        self.generation = generation;
        write_le_u32(&mut self.raw, 100, generation);
    }

    fn set_dev(&mut self, dev: u32) {
        self.block.fill(0);
        let offset = if dev & !0xffff != 0 { 4 } else { 0 };
        write_le_u32(&mut self.block, offset, dev);
        self.raw[40..100].copy_from_slice(&self.block);
    }

    fn set_extent_root(&mut self) {
        self.block.fill(0);
        write_extent_header(&mut self.block, 0, 4, 0);
        self.raw[40..100].copy_from_slice(&self.block);
        self.set_flags(self.flags | EXT4_INODE_FLAG_EXTENTS);
    }

    fn set_fast_symlink(&mut self, target: &[u8]) {
        self.block.fill(0);
        self.block[..target.len()].copy_from_slice(target);
        self.raw[40..100].copy_from_slice(&self.block);
        self.set_flags(self.flags & !EXT4_INODE_FLAG_EXTENTS);
        self.set_size(target.len() as u64);
        self.set_blocks_count(0);
    }

    fn add_blocks(&mut self, blocks: u64) -> SysResult<()> {
        self.blocks_count = self.blocks_count.checked_add(blocks).ok_or(Errno::EFBIG)?;
        write_le_u32(&mut self.raw, 28, self.blocks_count as u32);
        write_le_u16(&mut self.raw, 116, (self.blocks_count >> 32) as u16);
        Ok(())
    }

    fn subtract_blocks(&mut self, blocks: u64) -> SysResult<()> {
        self.set_blocks_count(self.blocks_count.checked_sub(blocks).ok_or(Errno::EIO)?);
        Ok(())
    }

    fn set_blocks_count(&mut self, blocks: u64) {
        self.blocks_count = blocks;
        write_le_u32(&mut self.raw, 28, blocks as u32);
        write_le_u16(&mut self.raw, 116, (blocks >> 32) as u16);
    }

    fn set_atime(&mut self, time: &Duration) {
        self.atime = encode_time(time);
        write_le_u32(&mut self.raw, 8, self.atime);
    }

    fn set_ctime(&mut self, time: &Duration) {
        self.ctime = encode_time(time);
        write_le_u32(&mut self.raw, 12, self.ctime);
    }

    fn set_mtime(&mut self, time: &Duration) {
        self.mtime = encode_time(time);
        write_le_u32(&mut self.raw, 16, self.mtime);
    }

    fn set_checksum(&mut self, checksum: u32, inode_size: u16) {
        write_le_u16(&mut self.raw, 124, checksum as u16);
        if inode_size > EXT4_MIN_INODE_SIZE {
            write_le_u16(&mut self.raw, 130, (checksum >> 16) as u16);
        }
    }
}

pub struct SuperBlock {
    inner: Arc<SleepLock<SuperBlockInner>>,
}

impl SuperBlock {
    pub fn new(driver: Arc<dyn BlockDriverOps>, read_only: bool) -> SysResult<Arc<Self>> {
        Ok(Arc::new(Self {
            inner: Arc::new(SleepLock::new(
                SuperBlockInner::new(driver, read_only)?,
                "ext4_native::SuperBlock::inner",
            )),
        }))
    }
}

impl SuperBlockOps for SuperBlock {
    fn get_root_ino(&self) -> u32 {
        ROOT_INO
    }

    fn create_temp(&self, _mode: Mode) -> SysResult<Arc<dyn InodeOps>> {
        if self.is_readonly() {
            Err(Errno::EROFS)
        } else {
            Err(Errno::EOPNOTSUPP)
        }
    }

    fn get_inode(&self, ino: u32) -> SysResult<Arc<dyn InodeOps>> {
        if !self.inner.lock().has_inode(ino) {
            return Err(Errno::ENOENT);
        }
        Ok(Arc::new(Inode::new(ino, self.inner.clone())?))
    }

    fn statfs(&self) -> SysResult<Statfs> {
        let inner = self.inner.lock();
        Ok(Statfs {
            f_type: EXT4_SUPER_MAGIC_STATFS,
            f_bsize: inner.info.block_size,
            f_blocks: inner.info.blocks_count,
            f_bfree: inner.info.free_blocks_count,
            f_bavail: inner.info.free_blocks_count,
            f_files: inner.info.inodes_count as u64,
            f_ffree: inner.info.free_inodes_count as u64,
            f_fsid: 0,
            f_namelen: EXT4_NAME_LEN as u64,
            f_frsize: inner.info.block_size,
            f_flag: if inner.is_readonly() {
                StatfsFlags::ST_RDONLY
            } else {
                StatfsFlags::empty()
            }
            .bits(),
            f_spare: [0; 4],
        })
    }

    fn is_readonly(&self) -> bool {
        self.inner.lock().is_readonly()
    }

    fn sync(&self) -> SysResult<()> {
        self.inner.lock().sync()
    }

    fn unmount(&self) -> SysResult<()> {
        self.sync()
    }

    fn type_name(&self) -> &'static str {
        "ext4native"
    }
}

fn insert_lru_cache<K: Ord + Copy, V>(cache: &mut LRUCache<K, V>, capacity: usize, key: K, value: V) {
    if capacity == 0 {
        return;
    }

    if !cache.contains_key(&key) {
        while cache.len() >= capacity {
            if cache.pop_lru().is_none() {
                break;
            }
        }
    }

    cache.put(key, value);
}

fn parse_extent_header(raw: &[u8]) -> SysResult<ExtentHeader> {
    if raw.len() < EXT4_EXTENT_HEADER_SIZE {
        return Err(Errno::EIO);
    }
    if le_u16(raw, 0) != EXT4_EXTENT_MAGIC {
        return Err(Errno::EOPNOTSUPP);
    }

    let header = ExtentHeader {
        entries: le_u16(raw, 2),
        max_entries: le_u16(raw, 4),
        depth: le_u16(raw, 6),
    };
    let capacity = (raw.len() - EXT4_EXTENT_HEADER_SIZE) / EXT4_EXTENT_ENTRY_SIZE;
    if header.entries > header.max_entries || header.max_entries as usize > capacity {
        return Err(Errno::EIO);
    }
    Ok(header)
}

fn parse_extent_index(raw: &[u8]) -> ExtentIndex {
    ExtentIndex {
        first_block: le_u32(raw, 0),
        leaf: le_u32(raw, 4) as u64 | ((le_u16(raw, 8) as u64) << 32),
    }
}

fn parse_extent(raw: &[u8]) -> Extent {
    let block_count = le_u16(raw, 4);
    let unwritten = block_count & EXT4_EXTENT_UNWRITTEN != 0;
    Extent {
        first_block: le_u32(raw, 0),
        len: block_count & !EXT4_EXTENT_UNWRITTEN,
        pblock: le_u32(raw, 8) as u64 | ((le_u16(raw, 6) as u64) << 32),
        unwritten,
    }
}

fn htree_entry_offset(node: HTreeNode, index: usize) -> SysResult<usize> {
    if index >= node.count as usize {
        return Err(Errno::EIO);
    }
    node.entry_offset
        .checked_add(index.checked_mul(EXT4_DX_ENTRY_SIZE).ok_or(Errno::EIO)?)
        .ok_or(Errno::EIO)
}

fn htree_entry_hash(block: &[u8], node: HTreeNode, index: usize) -> SysResult<u32> {
    Ok(le_u32(block, htree_entry_offset(node, index)?))
}

fn htree_entry_block(block: &[u8], node: HTreeNode, index: usize) -> SysResult<u32> {
    Ok(le_u32(block, htree_entry_offset(node, index)? + 4))
}

fn htree_insert_entry(
    block: &mut [u8],
    mut node: HTreeNode,
    position: usize,
    hash: u32,
    logical_block: u32,
) -> SysResult<HTreeNode> {
    if position >= node.count as usize || node.count >= node.limit {
        return Err(Errno::EIO);
    }

    let insert_index = position.checked_add(1).ok_or(Errno::EIO)?;
    let insert_offset = node
        .entry_offset
        .checked_add(insert_index.checked_mul(EXT4_DX_ENTRY_SIZE).ok_or(Errno::EIO)?)
        .ok_or(Errno::EIO)?;
    let old_end = node
        .entry_offset
        .checked_add(node.count as usize * EXT4_DX_ENTRY_SIZE)
        .ok_or(Errno::EIO)?;
    let new_end = old_end.checked_add(EXT4_DX_ENTRY_SIZE).ok_or(Errno::EIO)?;
    if new_end > block.len() {
        return Err(Errno::EIO);
    }

    block.copy_within(insert_offset..old_end, insert_offset + EXT4_DX_ENTRY_SIZE);
    write_le_u32(block, insert_offset, hash);
    write_le_u32(block, insert_offset + 4, logical_block);
    node.count += 1;
    write_le_u16(block, node.entry_offset + 2, node.count);
    Ok(node)
}

fn htree_hash_matches(block: &[u8], node: HTreeNode, index: usize, hash: u32) -> bool {
    htree_entry_hash(block, node, index).is_ok_and(|entry_hash| entry_hash & !1 == hash)
}

fn htree_find_position(block: &[u8], node: HTreeNode, hash: u32) -> usize {
    let mut low = 1usize;
    let mut high = node.count as usize;
    while low < high {
        let middle = low + (high - low) / 2;
        if htree_entry_hash(block, node, middle).unwrap_or(u32::MAX) > hash {
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
        if index > 0 && current_size.checked_add(entry.rec_len).ok_or(Errno::EIO)? > usable_len / 2 {
            return Ok(index);
        }
        current_size = current_size.checked_add(entry.rec_len).ok_or(Errno::EIO)?;
    }

    Ok(entries.len() - 1)
}

fn directory_block_offset(block_size: u64, logical_block: u32) -> SysResult<usize> {
    let offset = u64::from(logical_block).checked_mul(block_size).ok_or(Errno::EFBIG)?;
    usize::try_from(offset).map_err(|_| Errno::EFBIG)
}

fn htree_hash(name: &[u8], seed: [u32; 4], version: HTreeHashVersion) -> SysResult<u32> {
    if name.is_empty() || name.len() > EXT4_NAME_LEN {
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

fn le_u16(buf: &[u8], offset: usize) -> u16 {
    u16::from_le_bytes(buf[offset..offset + 2].try_into().unwrap())
}

fn le_u32(buf: &[u8], offset: usize) -> u32 {
    u32::from_le_bytes(buf[offset..offset + 4].try_into().unwrap())
}

fn write_le_u16(buf: &mut [u8], offset: usize, value: u16) {
    buf[offset..offset + 2].copy_from_slice(&value.to_le_bytes());
}

fn write_le_u32(buf: &mut [u8], offset: usize, value: u32) {
    buf[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
}

fn write_extent_header_entries(buf: &mut [u8], entries: u16) {
    write_le_u16(buf, 2, entries);
}

fn write_extent_header(buf: &mut [u8], entries: u16, max_entries: u16, depth: u16) {
    write_le_u16(buf, 0, EXT4_EXTENT_MAGIC);
    write_le_u16(buf, 2, entries);
    write_le_u16(buf, 4, max_entries);
    write_le_u16(buf, 6, depth);
    write_le_u32(buf, 8, 0);
}

fn write_extent_len(buf: &mut [u8], offset: usize, len: u16) {
    write_le_u16(buf, offset + 4, len);
}

fn write_extent_index(buf: &mut [u8], offset: usize, first_block: u32, leaf: u64) {
    write_le_u32(buf, offset, first_block);
    write_le_u32(buf, offset + 4, leaf as u32);
    write_le_u16(buf, offset + 8, (leaf >> 32) as u16);
    write_le_u16(buf, offset + 10, 0);
}

fn write_extent(buf: &mut [u8], offset: usize, extent: Extent) {
    write_le_u32(buf, offset, extent.first_block);
    write_le_u16(buf, offset + 4, extent.len);
    write_le_u16(buf, offset + 6, (extent.pblock >> 32) as u16);
    write_le_u32(buf, offset + 8, extent.pblock as u32);
}

fn dirent_min_len(name_len: usize) -> usize {
    (EXT4_DIR_ENTRY_HEADER_SIZE + name_len + 3) & !3
}

fn write_dirent(buf: &mut [u8], offset: usize, ino: u32, rec_len: usize, name: &[u8], file_type: u8) -> SysResult<()> {
    if rec_len > u16::MAX as usize || name.len() > u8::MAX as usize {
        return Err(Errno::EIO);
    }
    write_le_u32(buf, offset, ino);
    write_le_u16(buf, offset + 4, rec_len as u16);
    buf[offset + 6] = name.len() as u8;
    buf[offset + 7] = file_type;
    let name_start = offset + EXT4_DIR_ENTRY_HEADER_SIZE;
    let name_end = name_start + name.len();
    buf[name_start..name_end].copy_from_slice(name);
    buf[name_end..offset + rec_len].fill(0);
    Ok(())
}

fn dirent_tail_offset(block: &[u8]) -> Option<usize> {
    if block.len() < EXT4_DIRENT_TAIL_SIZE {
        return None;
    }
    let tail = block.len() - EXT4_DIRENT_TAIL_SIZE;
    (le_u32(block, tail) == 0
        && le_u16(block, tail + 4) as usize == EXT4_DIRENT_TAIL_SIZE
        && block[tail + 6] == 0
        && block[tail + 7] == EXT4_DIRENT_TAIL_FILE_TYPE)
        .then_some(tail)
}

fn bitmap_get(bitmap: &[u8], bit: u32) -> bool {
    let byte = bit as usize / 8;
    let mask = 1u8 << (bit % 8);
    bitmap.get(byte).map_or(true, |value| value & mask != 0)
}

fn bitmap_set(bitmap: &mut [u8], bit: u32) {
    let byte = bit as usize / 8;
    let mask = 1u8 << (bit % 8);
    bitmap[byte] |= mask;
}

fn bitmap_clear(bitmap: &mut [u8], bit: u32) {
    let byte = bit as usize / 8;
    let mask = 1u8 << (bit % 8);
    bitmap[byte] &= !mask;
}

fn now() -> Duration {
    kclock::now().unwrap_or(Duration::ZERO)
}

fn encode_time(time: &Duration) -> u32 {
    time.as_secs() as u32
}
