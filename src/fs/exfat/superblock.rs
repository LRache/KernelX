use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::sync::Arc;
use alloc::vec::Vec;
use core::{char, cmp};

use crate::driver::BlockDriverOps;
use crate::fs::file::DirResult;
use crate::fs::filesystem::SuperBlockOps;
use crate::fs::inode::Mode;
use crate::fs::{FileType, InodeOps};
use crate::kernel::errno::{Errno, SysResult};
use crate::kernel::uapi::{FileStat, Statfs};
use crate::klib::SleepLock;

use super::inode::Inode;

const BOOT_SECTOR_SIZE: usize = 512;
const BOOT_SIGNATURE_OFFSET: usize = 510;
const EXFAT_FS_NAME: &[u8; 8] = b"EXFAT   ";
const ROOT_INO: u32 = 1;
const DIR_ENTRY_SIZE: u64 = 32;
const FILE_ENTRY: u8 = 0x85;
const STREAM_ENTRY: u8 = 0xc0;
const FILENAME_ENTRY: u8 = 0xc1;
const ATTR_DIRECTORY: u16 = 0x10;
const STREAM_NO_FAT_CHAIN: u8 = 0x02;
const MIN_CLUSTER: u32 = 2;
const BAD_CLUSTER: u32 = 0xffff_fff7;
const END_CLUSTER: u32 = 0xffff_fff8;
const EXFAT_SUPER_MAGIC: u64 = 0x2011_bab0;

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
    no_fat_chain: bool,
}

struct DirectoryRecord {
    key: InodeKey,
    parent_ino: u32,
    first_cluster: u32,
    data_length: u64,
    is_dir: bool,
    no_fat_chain: bool,
    name: String,
}

#[derive(Clone, Copy)]
struct ExfatInfo {
    fat_offset: u64,
    cluster_heap_offset: u64,
    cluster_count: u32,
    root_cluster: u32,
    sector_size: u64,
    sectors_per_cluster: u64,
    cluster_size: u64,
}

pub(super) struct SuperBlockInner {
    driver: Arc<dyn BlockDriverOps>,
    info: ExfatInfo,
    next_ino: u32,
    inode_by_ino: BTreeMap<u32, InodeMeta>,
    ino_by_key: BTreeMap<InodeKey, u32>,
}

impl SuperBlockInner {
    fn new(driver: Arc<dyn BlockDriverOps>) -> SysResult<Self> {
        let info = ExfatInfo::read_from(driver.clone())?;
        let root = InodeMeta {
            ino: ROOT_INO,
            key: InodeKey::Root,
            parent_ino: ROOT_INO,
            first_cluster: info.root_cluster,
            data_length: 0,
            is_dir: true,
            no_fat_chain: false,
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
        let mut stat = FileStat::default();
        stat.st_ino = ino as u64;
        stat.st_mode = self.inode_mode(ino)?.bits();
        stat.st_nlink = if meta.is_dir { 2 } else { 1 };
        stat.st_size = if meta.is_dir {
            self.directory_size(&meta) as i64
        } else {
            meta.data_length as i64
        };
        stat.st_blksize = self.info.cluster_size as i32;
        stat.st_blocks = if meta.is_dir {
            self.directory_size(&meta).div_ceil(512)
        } else {
            meta.data_length.div_ceil(512)
        };
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
            if record.name == name {
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
                cluster = if meta.no_fat_chain {
                    cluster.checked_add(1).ok_or(Errno::EIO)?
                } else {
                    self.next_cluster(cluster)?.ok_or(Errno::EIO)?
                };
                self.check_cluster(cluster)?;
            }
        }

        Ok(copied)
    }

    fn ensure_inode(&mut self, record: DirectoryRecord) -> u32 {
        if let Some(&ino) = self.ino_by_key.get(&record.key) {
            return ino;
        }

        let ino = self.next_ino;
        self.next_ino = self.next_ino.checked_add(1).expect("exfat inode number overflow");
        let meta = InodeMeta {
            ino,
            key: record.key,
            parent_ino: record.parent_ino,
            first_cluster: record.first_cluster,
            data_length: record.data_length,
            is_dir: record.is_dir,
            no_fat_chain: record.no_fat_chain,
        };
        self.ino_by_key.insert(meta.key, ino);
        self.inode_by_ino.insert(ino, meta);
        ino
    }

    fn read_directory_records(&self, dir: &InodeMeta) -> SysResult<Vec<DirectoryRecord>> {
        let mut records = Vec::new();
        let mut index = 0usize;

        while let Some((entry, entry_offset)) = self.read_directory_entry(dir, index)? {
            match entry[0] {
                0x00 => break,
                FILE_ENTRY => {
                    let secondary_count = entry[1] as usize;
                    if secondary_count == 0 {
                        index += 1;
                        continue;
                    }

                    if let Some(record) =
                        self.parse_file_entry_set(dir, &entry, entry_offset, index, secondary_count)?
                    {
                        records.push(record);
                    }
                    index += secondary_count + 1;
                }
                _ => index += 1,
            }
        }

        Ok(records)
    }

    fn parse_file_entry_set(
        &self,
        dir: &InodeMeta,
        file_entry: &[u8; 32],
        entry_offset: u64,
        entry_index: usize,
        secondary_count: usize,
    ) -> SysResult<Option<DirectoryRecord>> {
        let mut stream = None;
        let mut name_units = Vec::new();

        for secondary_index in 0..secondary_count {
            let Some((entry, _)) = self.read_directory_entry(dir, entry_index + secondary_index + 1)? else {
                return Err(Errno::EIO);
            };

            match entry[0] {
                STREAM_ENTRY => {
                    stream = Some(StreamEntry {
                        no_fat_chain: entry[1] & STREAM_NO_FAT_CHAIN != 0,
                        name_len: entry[3],
                        first_cluster: le_u32(&entry, 20),
                        data_length: le_u64(&entry, 24),
                    });
                }
                FILENAME_ENTRY => {
                    for unit_index in 0..15 {
                        let offset = 2 + unit_index * 2;
                        name_units.push(le_u16(&entry, offset));
                    }
                }
                _ => {}
            }
        }

        let Some(stream) = stream else {
            return Ok(None);
        };
        let name_len = stream.name_len as usize;
        if name_len > name_units.len() {
            return Err(Errno::EIO);
        }

        let name = decode_name(&name_units[..name_len])?;
        if name.is_empty() {
            return Ok(None);
        }

        let is_dir = le_u16(file_entry, 4) & ATTR_DIRECTORY != 0;
        Ok(Some(DirectoryRecord {
            key: InodeKey::Entry {
                parent_cluster: dir.first_cluster,
                entry_offset,
            },
            parent_ino: dir.ino,
            first_cluster: stream.first_cluster,
            data_length: stream.data_length,
            is_dir,
            no_fat_chain: stream.no_fat_chain,
            name,
        }))
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
        if meta.no_fat_chain {
            let cluster = meta
                .first_cluster
                .checked_add(u32::try_from(cluster_index).map_err(|_| Errno::EIO)?)
                .ok_or(Errno::EIO)?;
            self.check_cluster(cluster)?;
            return Ok(Some(cluster));
        }

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
        let next = u32::from_le_bytes(raw);
        match next {
            BAD_CLUSTER => Err(Errno::EIO),
            END_CLUSTER..=u32::MAX => Ok(None),
            MIN_CLUSTER.. => {
                self.check_cluster(next)?;
                Ok(Some(next))
            }
            _ => Err(Errno::EIO),
        }
    }

    fn cluster_offset(&self, cluster: u32) -> SysResult<u64> {
        self.check_cluster(cluster)?;
        let cluster_index = cluster.checked_sub(MIN_CLUSTER).ok_or(Errno::EIO)? as u64;
        let sector = self
            .info
            .cluster_heap_offset
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

struct StreamEntry {
    no_fat_chain: bool,
    name_len: u8,
    first_cluster: u32,
    data_length: u64,
}

impl ExfatInfo {
    fn read_from(driver: Arc<dyn BlockDriverOps>) -> SysResult<Self> {
        let mut boot = [0u8; BOOT_SECTOR_SIZE];
        driver.read_at(0, &mut boot).map_err(|_| Errno::EIO)?;

        if &boot[3..11] != EXFAT_FS_NAME {
            return Err(Errno::EINVAL);
        }
        if boot[BOOT_SIGNATURE_OFFSET] != 0x55 || boot[BOOT_SIGNATURE_OFFSET + 1] != 0xaa {
            return Err(Errno::EINVAL);
        }

        let fat_offset = le_u32(&boot, 80) as u64;
        let fat_length = le_u32(&boot, 84) as u64;
        let cluster_heap_offset = le_u32(&boot, 88) as u64;
        let cluster_count = le_u32(&boot, 92);
        let root_cluster = le_u32(&boot, 96);
        let volume_length = le_u64(&boot, 72);
        let sector_shift = boot[108];
        let sectors_per_cluster_shift = boot[109];
        let number_of_fats = boot[110];

        if !(9..=12).contains(&sector_shift) || sectors_per_cluster_shift > 25 {
            return Err(Errno::EINVAL);
        }
        if fat_offset == 0 || fat_length == 0 || cluster_heap_offset == 0 || cluster_count == 0 || number_of_fats == 0 {
            return Err(Errno::EINVAL);
        }

        let sector_size = 1u64.checked_shl(sector_shift as u32).ok_or(Errno::EINVAL)?;
        let sectors_per_cluster = 1u64
            .checked_shl(sectors_per_cluster_shift as u32)
            .ok_or(Errno::EINVAL)?;
        let cluster_size = sector_size.checked_mul(sectors_per_cluster).ok_or(Errno::EINVAL)?;

        let all_fats_end = fat_offset
            .checked_add(fat_length.checked_mul(number_of_fats as u64).ok_or(Errno::EINVAL)?)
            .ok_or(Errno::EINVAL)?;
        let fat_bytes = fat_length.checked_mul(sector_size).ok_or(Errno::EINVAL)?;
        let needed_fat_bytes = (cluster_count as u64 + MIN_CLUSTER as u64)
            .checked_mul(4)
            .ok_or(Errno::EINVAL)?;
        let cluster_heap_end = cluster_heap_offset
            .checked_add(
                (cluster_count as u64)
                    .checked_mul(sectors_per_cluster)
                    .ok_or(Errno::EINVAL)?,
            )
            .ok_or(Errno::EINVAL)?;
        if fat_bytes < needed_fat_bytes || all_fats_end > cluster_heap_offset || cluster_heap_end > volume_length {
            return Err(Errno::EINVAL);
        }

        let device_bytes = (driver.get_block_size() as u64)
            .checked_mul(driver.get_block_count())
            .ok_or(Errno::EINVAL)?;
        let volume_bytes = volume_length.checked_mul(sector_size).ok_or(Errno::EINVAL)?;
        if volume_bytes > device_bytes {
            return Err(Errno::EINVAL);
        }

        let info = Self {
            fat_offset,
            cluster_heap_offset,
            cluster_count,
            root_cluster,
            sector_size,
            sectors_per_cluster,
            cluster_size,
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
            inner: Arc::new(SleepLock::new(
                SuperBlockInner::new(driver)?,
                "exfat::SuperBlock::inner",
            )),
        }))
    }
}

impl SuperBlockOps for SuperBlock {
    fn get_root_ino(&self) -> u32 {
        ROOT_INO
    }

    fn get_inode(&self, ino: u32) -> SysResult<Arc<dyn InodeOps>> {
        if !self.inner.lock().has_inode(ino) {
            return Err(Errno::ENOENT);
        }
        Ok(Arc::new(Inode::new(ino, self.inner.clone())))
    }

    fn statfs(&self) -> SysResult<Statfs> {
        let inner = self.inner.lock();
        Ok(Statfs {
            f_type: EXFAT_SUPER_MAGIC,
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
        "exfat"
    }
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

fn le_u16(buf: &[u8], offset: usize) -> u16 {
    u16::from_le_bytes(buf[offset..offset + 2].try_into().unwrap())
}

fn le_u32(buf: &[u8], offset: usize) -> u32 {
    u32::from_le_bytes(buf[offset..offset + 4].try_into().unwrap())
}

fn le_u64(buf: &[u8], offset: usize) -> u64 {
    u64::from_le_bytes(buf[offset..offset + 8].try_into().unwrap())
}
