use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::sync::Arc;
use alloc::vec::Vec;
use core::{char, cmp};

use bitflags::bitflags;

use crate::driver::BlockDriverOps;
use crate::fs::FileType;
use crate::fs::file::DirResult;
use crate::fs::filesystem::SuperBlockOps;
use crate::fs::inode::Mode;
use crate::kernel::errno::{Errno, SysResult};
use crate::kernel::uapi::{FileStat, Statfs};
use crate::klib::SleepLock;

use super::inode::Inode as VfatInode;

const BOOT_SECTOR_SIZE: usize = 512;
const BOOT_SIGNATURE_OFFSET: usize = 510;
const FAT32_MIN_CLUSTER_COUNT: u64 = 65_525;
const ROOT_INO: u32 = 1;
const DIR_ENTRY_SIZE: u64 = 32;
const MIN_CLUSTER: u32 = 2;
const MAX_DATA_CLUSTER: u32 = 0x0fff_fff6;
const BAD_CLUSTER: u32 = 0x0fff_fff7;
const END_CLUSTER: u32 = 0x0fff_fff8;
const FAT_ENTRY_MASK: u32 = 0x0fff_ffff;
const VFAT_SUPER_MAGIC: u64 = 0x4d44;
const DELETED_ENTRY: u8 = 0xe5;
const END_ENTRY: u8 = 0x00;
const LFN_LAST_ENTRY: u8 = 0x40;
const LFN_ORDER_MASK: u8 = 0x1f;
const MAX_LFN_ENTRIES: u8 = 20;

bitflags! {
    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    struct DirAttr: u8 {
        const READ_ONLY = 0x01;
        const HIDDEN = 0x02;
        const SYSTEM = 0x04;
        const VOLUME_ID = 0x08;
        const DIRECTORY = 0x10;
        const ARCHIVE = 0x20;
        const LONG_NAME = Self::READ_ONLY.bits()
            | Self::HIDDEN.bits()
            | Self::SYSTEM.bits()
            | Self::VOLUME_ID.bits();
    }
}

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum InodeKey {
    Root,
    Entry { parent_cluster: u32, entry_offset: u64 },
}

#[derive(Clone)]
pub(super) struct InodeMeta {
    ino: u32,
    key: InodeKey,
    parent_ino: u32,
    first_cluster: u32,
    data_length: u64,
    is_dir: bool,
}

struct DirectoryRecord {
    key: InodeKey,
    parent_ino: u32,
    first_cluster: u32,
    data_length: u64,
    is_dir: bool,
    name: String,
}

#[derive(Clone, Copy)]
struct FatInfo {
    fat_offset: u64,
    data_offset: u64,
    cluster_count: u32,
    root_cluster: u32,
    sector_size: u64,
    sectors_per_cluster: u64,
    cluster_size: u64,
}

#[derive(Clone, Copy)]
struct LfnPart {
    order: u8,
    checksum: u8,
    is_last: bool,
    units: [u16; 13],
}

pub(super) struct SuperBlockInner {
    driver: Arc<dyn BlockDriverOps>,
    info: FatInfo,
    next_ino: u32,
    inode_by_ino: BTreeMap<u32, InodeMeta>,
    ino_by_key: BTreeMap<InodeKey, u32>,
}

impl SuperBlockInner {
    fn new(driver: Arc<dyn BlockDriverOps>) -> SysResult<Self> {
        let info = FatInfo::read_from(driver.clone())?;
        let root = InodeMeta {
            ino: ROOT_INO,
            key: InodeKey::Root,
            parent_ino: ROOT_INO,
            first_cluster: info.root_cluster,
            data_length: 0,
            is_dir: true,
        };

        let mut inode_by_ino = BTreeMap::new();
        inode_by_ino.insert(ROOT_INO, root);

        let mut ino_by_key = BTreeMap::new();
        ino_by_key.insert(InodeKey::Root, ROOT_INO);

        Ok(Self {
            driver,
            info,
            next_ino: ROOT_INO + 1,
            inode_by_ino,
            ino_by_key,
        })
    }

    fn get_meta(&self, ino: u32) -> SysResult<InodeMeta> {
        self.inode_by_ino.get(&ino).cloned().ok_or(Errno::ENOENT)
    }

    pub fn has_inode(&self, ino: u32) -> bool {
        self.inode_by_ino.contains_key(&ino)
    }

    pub fn inode_size(&self, ino: u32) -> SysResult<u64> {
        let meta = self.get_meta(ino)?;
        if meta.is_dir {
            Ok(self.directory_size(&meta))
        } else {
            Ok(meta.data_length)
        }
    }

    pub fn inode_mode(&self, ino: u32) -> SysResult<Mode> {
        let meta = self.get_meta(ino)?;
        let file_type = if meta.is_dir { Mode::S_IFDIR } else { Mode::S_IFREG };
        Ok(file_type | Mode::from_bits_truncate(0o555))
    }

    pub fn inode_type(&self, ino: u32) -> SysResult<FileType> {
        if self.get_meta(ino)?.is_dir {
            Ok(FileType::Directory)
        } else {
            Ok(FileType::Regular)
        }
    }

    pub fn inode_stat(&self, ino: u32) -> SysResult<FileStat> {
        let meta = self.get_meta(ino)?;
        let size = if meta.is_dir {
            self.directory_size(&meta)
        } else {
            meta.data_length
        };

        let mut stat = FileStat::default();
        stat.st_ino = ino as u64;
        stat.st_mode = self.inode_mode(ino)?.bits();
        stat.st_nlink = if meta.is_dir { 2 } else { 1 };
        stat.st_size = size as i64;
        stat.st_blksize = self.info.cluster_size as i32;
        stat.st_blocks = size.div_ceil(512);
        Ok(stat)
    }

    fn directory_size(&self, meta: &InodeMeta) -> u64 {
        if meta.data_length == 0 {
            self.info.cluster_size
        } else {
            meta.data_length
        }
    }

    pub fn lookup(&mut self, ino: u32, name: &str) -> SysResult<u32> {
        let meta = self.get_meta(ino)?;
        if !meta.is_dir {
            return Err(Errno::ENOTDIR);
        }

        match name {
            "." => return Ok(meta.ino),
            ".." => return Ok(meta.parent_ino),
            _ => {}
        }

        for record in self.read_directory_records(&meta)? {
            if names_match(&record.name, name) {
                return Ok(self.ensure_inode(record));
            }
        }
        Err(Errno::ENOENT)
    }

    pub fn get_dent(&mut self, ino: u32, index: usize) -> SysResult<Option<(DirResult, usize)>> {
        let meta = self.get_meta(ino)?;
        if !meta.is_dir {
            return Err(Errno::ENOTDIR);
        }

        if index == 0 {
            return Ok(Some((
                DirResult {
                    ino: meta.ino,
                    name: String::from("."),
                    file_type: FileType::Directory,
                },
                1,
            )));
        }

        if index == 1 {
            return Ok(Some((
                DirResult {
                    ino: meta.parent_ino,
                    name: String::from(".."),
                    file_type: FileType::Directory,
                },
                2,
            )));
        }

        let records = self.read_directory_records(&meta)?;
        let Some(record) = records.into_iter().nth(index - 2) else {
            return Ok(None);
        };
        let file_type = if record.is_dir {
            FileType::Directory
        } else {
            FileType::Regular
        };
        let name = record.name.clone();
        let ino = self.ensure_inode(record);
        Ok(Some((DirResult { ino, name, file_type }, index + 1)))
    }

    pub fn read_inode(&self, ino: u32, buf: &mut [u8], offset: usize) -> SysResult<usize> {
        let meta = self.get_meta(ino)?;
        if meta.is_dir {
            return Err(Errno::EISDIR);
        }
        if buf.is_empty() {
            return Ok(0);
        }

        let offset = offset as u64;
        if offset >= meta.data_length {
            return Ok(0);
        }

        let mut copied = 0usize;
        let mut current = offset;
        let read_len = cmp::min(buf.len() as u64, meta.data_length - offset) as usize;
        let mut cluster = self.cluster_for_logical_offset(&meta, current)?.ok_or(Errno::EIO)?;

        while copied < read_len {
            let cluster_offset = current % self.info.cluster_size;
            let copy_len = cmp::min(read_len - copied, (self.info.cluster_size - cluster_offset) as usize);
            let device_offset = self.cluster_offset(cluster)? + cluster_offset;
            self.read_at(device_offset, &mut buf[copied..copied + copy_len])?;

            copied += copy_len;
            current += copy_len as u64;
            if copied < read_len {
                cluster = self.next_cluster(cluster)?.ok_or(Errno::EIO)?;
            }
        }

        Ok(copied)
    }

    fn ensure_inode(&mut self, record: DirectoryRecord) -> u32 {
        if let Some(&ino) = self.ino_by_key.get(&record.key) {
            return ino;
        }

        let ino = self.next_ino;
        self.next_ino = self.next_ino.checked_add(1).expect("vfat inode number overflow");
        let meta = InodeMeta {
            ino,
            key: record.key,
            parent_ino: record.parent_ino,
            first_cluster: record.first_cluster,
            data_length: record.data_length,
            is_dir: record.is_dir,
        };
        self.ino_by_key.insert(meta.key, ino);
        self.inode_by_ino.insert(ino, meta);
        ino
    }

    fn read_directory_records(&self, dir: &InodeMeta) -> SysResult<Vec<DirectoryRecord>> {
        let mut records = Vec::new();
        let mut lfn_parts = Vec::new();
        let mut index = 0usize;

        while let Some((entry, entry_offset)) = self.read_directory_entry(dir, index)? {
            match entry[0] {
                END_ENTRY => break,
                DELETED_ENTRY => {
                    lfn_parts.clear();
                    index += 1;
                    continue;
                }
                _ => {}
            }

            if is_lfn_entry(&entry) {
                match parse_lfn_entry(&entry) {
                    Some(part) => {
                        if part.is_last {
                            lfn_parts.clear();
                        }
                        lfn_parts.push(part);
                    }
                    None => lfn_parts.clear(),
                }
                index += 1;
                continue;
            }

            let attr = DirAttr::from_bits_truncate(entry[11]);
            if attr.contains(DirAttr::VOLUME_ID) {
                lfn_parts.clear();
                index += 1;
                continue;
            }

            if let Some(record) = self.parse_short_entry(dir, &entry, entry_offset, &lfn_parts) {
                records.push(record);
            }
            lfn_parts.clear();
            index += 1;
        }

        Ok(records)
    }

    fn parse_short_entry(
        &self,
        dir: &InodeMeta,
        entry: &[u8; 32],
        entry_offset: u64,
        lfn_parts: &[LfnPart],
    ) -> Option<DirectoryRecord> {
        let short_name = decode_short_name(entry)?;
        if short_name == "." || short_name == ".." {
            return None;
        }

        let name = decode_lfn_name(lfn_parts, short_name_checksum(&entry[..11])).unwrap_or(short_name);
        let attr = DirAttr::from_bits_truncate(entry[11]);
        let first_cluster = ((le_u16(entry, 20) as u32) << 16) | le_u16(entry, 26) as u32;
        Some(DirectoryRecord {
            key: InodeKey::Entry {
                parent_cluster: dir.first_cluster,
                entry_offset,
            },
            parent_ino: dir.ino,
            first_cluster,
            data_length: le_u32(entry, 28) as u64,
            is_dir: attr.contains(DirAttr::DIRECTORY),
            name,
        })
    }

    fn read_directory_entry(&self, dir: &InodeMeta, index: usize) -> SysResult<Option<([u8; 32], u64)>> {
        let logical_offset = (index as u64).checked_mul(DIR_ENTRY_SIZE).ok_or(Errno::EIO)?;
        if dir.data_length != 0 && logical_offset + DIR_ENTRY_SIZE > dir.data_length {
            return Ok(None);
        }

        let Some(cluster) = self.cluster_for_logical_offset(dir, logical_offset)? else {
            return Ok(None);
        };
        let cluster_offset = logical_offset % self.info.cluster_size;
        if cluster_offset + DIR_ENTRY_SIZE > self.info.cluster_size {
            return Err(Errno::EIO);
        }

        let device_offset = self.cluster_offset(cluster)? + cluster_offset;
        let mut entry = [0u8; 32];
        self.read_at(device_offset, &mut entry)?;
        Ok(Some((entry, device_offset)))
    }

    fn cluster_for_logical_offset(&self, meta: &InodeMeta, logical_offset: u64) -> SysResult<Option<u32>> {
        if meta.first_cluster == 0 {
            return Ok(None);
        }

        let cluster_index = logical_offset / self.info.cluster_size;
        let mut cluster = meta.first_cluster;
        self.check_cluster(cluster)?;
        for _ in 0..cluster_index {
            let Some(next) = self.next_cluster(cluster)? else {
                return Ok(None);
            };
            cluster = next;
        }
        Ok(Some(cluster))
    }

    fn next_cluster(&self, cluster: u32) -> SysResult<Option<u32>> {
        self.check_cluster(cluster)?;
        let fat_offset = self
            .info
            .fat_offset
            .checked_mul(self.info.sector_size)
            .and_then(|offset| offset.checked_add(cluster as u64 * 4))
            .ok_or(Errno::EIO)?;
        let mut raw = [0u8; 4];
        self.read_at(fat_offset, &mut raw)?;
        let next = u32::from_le_bytes(raw) & FAT_ENTRY_MASK;
        match next {
            0 | 1 => Err(Errno::EIO),
            BAD_CLUSTER => Err(Errno::EIO),
            END_CLUSTER..=FAT_ENTRY_MASK => Ok(None),
            MIN_CLUSTER.. => {
                self.check_cluster(next)?;
                Ok(Some(next))
            }
        }
    }

    fn cluster_offset(&self, cluster: u32) -> SysResult<u64> {
        self.check_cluster(cluster)?;
        let cluster_index = cluster.checked_sub(MIN_CLUSTER).ok_or(Errno::EIO)? as u64;
        let sector = self
            .info
            .data_offset
            .checked_add(
                cluster_index
                    .checked_mul(self.info.sectors_per_cluster)
                    .ok_or(Errno::EIO)?,
            )
            .ok_or(Errno::EIO)?;
        sector.checked_mul(self.info.sector_size).ok_or(Errno::EIO)
    }

    fn check_cluster(&self, cluster: u32) -> SysResult<()> {
        let max_cluster = self.info.cluster_count.checked_add(1).ok_or(Errno::EIO)?;
        if (MIN_CLUSTER..=max_cluster).contains(&cluster) {
            Ok(())
        } else {
            Err(Errno::EIO)
        }
    }

    fn read_at(&self, offset: u64, buf: &mut [u8]) -> SysResult<()> {
        let offset = usize::try_from(offset).map_err(|_| Errno::EIO)?;
        self.driver.read_at(offset, buf).map_err(|_| Errno::EIO)
    }
}

impl FatInfo {
    fn read_from(driver: Arc<dyn BlockDriverOps>) -> SysResult<Self> {
        let mut boot = [0u8; BOOT_SECTOR_SIZE];
        driver.read_at(0, &mut boot).map_err(|_| Errno::EIO)?;

        if boot[BOOT_SIGNATURE_OFFSET] != 0x55 || boot[BOOT_SIGNATURE_OFFSET + 1] != 0xaa {
            return Err(Errno::EINVAL);
        }

        let sector_size = le_u16(&boot, 11) as u64;
        let sectors_per_cluster = boot[13] as u64;
        let reserved_sectors = le_u16(&boot, 14) as u64;
        let number_of_fats = boot[16] as u64;
        let root_entries = le_u16(&boot, 17);
        let total_sectors_16 = le_u16(&boot, 19) as u64;
        let fat_size_16 = le_u16(&boot, 22);
        let total_sectors_32 = le_u32(&boot, 32) as u64;
        let fat_size = le_u32(&boot, 36) as u64;
        let ext_flags = le_u16(&boot, 40);
        let root_cluster = le_u32(&boot, 44);

        if sector_size != BOOT_SECTOR_SIZE as u64 || sectors_per_cluster == 0 || !sectors_per_cluster.is_power_of_two()
        {
            return Err(Errno::EINVAL);
        }
        if reserved_sectors == 0 || number_of_fats == 0 || fat_size == 0 || root_cluster < MIN_CLUSTER {
            return Err(Errno::EINVAL);
        }
        if root_entries != 0 || fat_size_16 != 0 {
            return Err(Errno::EINVAL);
        }

        let total_sectors = if total_sectors_16 != 0 {
            total_sectors_16
        } else {
            total_sectors_32
        };
        if total_sectors == 0 {
            return Err(Errno::EINVAL);
        }

        let active_fat = if ext_flags & 0x80 != 0 {
            (ext_flags & 0x000f) as u64
        } else {
            0
        };
        if active_fat >= number_of_fats {
            return Err(Errno::EINVAL);
        }

        let all_fats_size = number_of_fats.checked_mul(fat_size).ok_or(Errno::EINVAL)?;
        let data_offset = reserved_sectors.checked_add(all_fats_size).ok_or(Errno::EINVAL)?;
        if data_offset >= total_sectors {
            return Err(Errno::EINVAL);
        }

        let data_sectors = total_sectors - data_offset;
        let cluster_count = data_sectors / sectors_per_cluster;
        if !(FAT32_MIN_CLUSTER_COUNT..=MAX_DATA_CLUSTER as u64 - 1).contains(&cluster_count) {
            return Err(Errno::EINVAL);
        }

        let fat_bytes = fat_size.checked_mul(sector_size).ok_or(Errno::EINVAL)?;
        let needed_fat_bytes = cluster_count
            .checked_add(MIN_CLUSTER as u64)
            .and_then(|entries| entries.checked_mul(4))
            .ok_or(Errno::EINVAL)?;
        if fat_bytes < needed_fat_bytes {
            return Err(Errno::EINVAL);
        }

        let volume_bytes = total_sectors.checked_mul(sector_size).ok_or(Errno::EINVAL)?;
        let device_bytes = (driver.get_block_size() as u64)
            .checked_mul(driver.get_block_count())
            .ok_or(Errno::EINVAL)?;
        if volume_bytes > device_bytes {
            return Err(Errno::EINVAL);
        }

        let info = Self {
            fat_offset: reserved_sectors
                .checked_add(active_fat.checked_mul(fat_size).ok_or(Errno::EINVAL)?)
                .ok_or(Errno::EINVAL)?,
            data_offset,
            cluster_count: u32::try_from(cluster_count).map_err(|_| Errno::EINVAL)?,
            root_cluster,
            sector_size,
            sectors_per_cluster,
            cluster_size: sector_size.checked_mul(sectors_per_cluster).ok_or(Errno::EINVAL)?,
        };
        if info.check_root_cluster().is_err() {
            return Err(Errno::EINVAL);
        }
        Ok(info)
    }

    fn check_root_cluster(&self) -> SysResult<()> {
        let max_cluster = self.cluster_count.checked_add(1).ok_or(Errno::EINVAL)?;
        if (MIN_CLUSTER..=max_cluster).contains(&self.root_cluster) {
            Ok(())
        } else {
            Err(Errno::EINVAL)
        }
    }
}

pub struct SuperBlock {
    inner: Arc<SleepLock<SuperBlockInner>>,
}

impl SuperBlock {
    pub fn new(driver: Arc<dyn BlockDriverOps>) -> SysResult<Arc<Self>> {
        Ok(Arc::new(Self {
            inner: Arc::new(SleepLock::new(SuperBlockInner::new(driver)?, "vfat::SuperBlock::inner")),
        }))
    }
}

impl SuperBlockOps for SuperBlock {
    type Inode = VfatInode;

    fn get_root_ino(&self) -> u32 {
        ROOT_INO
    }

    fn get_inode(&self, ino: u32) -> SysResult<Self::Inode> {
        if !self.inner.lock().has_inode(ino) {
            return Err(Errno::ENOENT);
        }
        Ok(VfatInode::new(ino, self.inner.clone()))
    }

    fn statfs(&self) -> SysResult<Statfs> {
        let inner = self.inner.lock();
        Ok(Statfs {
            f_type: VFAT_SUPER_MAGIC,
            f_bsize: inner.info.cluster_size,
            f_blocks: inner.info.cluster_count as u64,
            f_bfree: 0,
            f_bavail: 0,
            f_files: inner.inode_by_ino.len() as u64,
            f_ffree: 0,
            f_fsid: 0,
            f_namelen: 255,
            f_frsize: inner.info.cluster_size,
            f_flag: self.statfs_flags().bits(),
            f_spare: [0; 4],
        })
    }

    fn is_readonly(&self) -> bool {
        true
    }

    fn type_name(&self) -> &'static str {
        "vfat"
    }
}

fn names_match(entry_name: &str, lookup_name: &str) -> bool {
    entry_name == lookup_name || entry_name.eq_ignore_ascii_case(lookup_name)
}

fn is_lfn_entry(entry: &[u8; 32]) -> bool {
    entry[11] == DirAttr::LONG_NAME.bits()
}

fn parse_lfn_entry(entry: &[u8; 32]) -> Option<LfnPart> {
    if entry[12] != 0 || le_u16(entry, 26) != 0 {
        return None;
    }

    let order = entry[0] & LFN_ORDER_MASK;
    if order == 0 || order > MAX_LFN_ENTRIES {
        return None;
    }

    let mut units = [0u16; 13];
    for (i, unit) in units.iter_mut().take(5).enumerate() {
        *unit = le_u16(entry, 1 + i * 2);
    }
    for (i, unit) in units.iter_mut().skip(5).take(6).enumerate() {
        *unit = le_u16(entry, 14 + i * 2);
    }
    for (i, unit) in units.iter_mut().skip(11).take(2).enumerate() {
        *unit = le_u16(entry, 28 + i * 2);
    }

    Some(LfnPart {
        order,
        checksum: entry[13],
        is_last: entry[0] & LFN_LAST_ENTRY != 0,
        units,
    })
}

fn decode_lfn_name(parts: &[LfnPart], checksum: u8) -> Option<String> {
    if parts.is_empty() || parts.iter().any(|part| part.checksum != checksum) {
        return None;
    }

    let expected_parts = parts.iter().find(|part| part.is_last)?.order as usize;
    if expected_parts == 0 || expected_parts != parts.len() {
        return None;
    }

    let mut units = Vec::new();
    for order in 1..=expected_parts {
        let part = parts.iter().find(|part| part.order as usize == order)?;
        for &unit in &part.units {
            match unit {
                0x0000 => return decode_name(&units).ok().filter(|name| !name.is_empty()),
                0xffff => {}
                _ => units.push(unit),
            }
        }
    }
    decode_name(&units).ok().filter(|name| !name.is_empty())
}

fn decode_name(units: &[u16]) -> SysResult<String> {
    let mut name = String::new();
    for item in char::decode_utf16(units.iter().copied()) {
        match item {
            Ok(ch) => name.push(ch),
            Err(_) => return Err(Errno::EIO),
        }
    }
    Ok(name)
}

fn decode_short_name(entry: &[u8; 32]) -> Option<String> {
    let mut base = short_name_part(&entry[..8], entry[12] & 0x08 != 0);
    if base.is_empty() {
        return None;
    }

    let ext = short_name_part(&entry[8..11], entry[12] & 0x10 != 0);
    if !ext.is_empty() {
        base.push('.');
        base.push_str(&ext);
    }
    Some(base)
}

fn short_name_part(bytes: &[u8], lowercase: bool) -> String {
    let end = bytes
        .iter()
        .rposition(|&byte| byte != b' ')
        .map_or(0, |index| index + 1);
    let mut name = String::new();
    for (index, &byte) in bytes[..end].iter().enumerate() {
        let byte = if index == 0 && byte == 0x05 {
            DELETED_ENTRY
        } else {
            byte
        };
        let byte = if lowercase { byte.to_ascii_lowercase() } else { byte };
        name.push(byte as char);
    }
    name
}

fn short_name_checksum(name: &[u8]) -> u8 {
    let mut sum = 0u8;
    for &byte in name {
        sum = ((sum & 1) << 7).wrapping_add(sum >> 1).wrapping_add(byte);
    }
    sum
}

fn le_u16(buf: &[u8], offset: usize) -> u16 {
    u16::from_le_bytes(buf[offset..offset + 2].try_into().unwrap())
}

fn le_u32(buf: &[u8], offset: usize) -> u32 {
    u32::from_le_bytes(buf[offset..offset + 4].try_into().unwrap())
}
