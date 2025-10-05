use alloc::sync::Arc;
use alloc::vec::Vec;
use ext4_rs::{Ext4, Ext4DirEntry, Ext4DirSearchResult, InodeFileType};

use crate::kernel::errno::{Errno, SysResult};
use crate::fs::inode::{Inode, Mode};
use crate::fs::file::{DirResult, FileType};

fn error_map(e: ext4_rs::Ext4Error) -> Errno {
    match e.error() {
        ext4_rs::Errno::EIO    => Errno::EIO,
        ext4_rs::Errno::EEXIST => Errno::EEXIST,
        ext4_rs::Errno::ENOENT => Errno::ENOENT,
        _ => Errno::EIO,
    }
}

pub struct Ext4Inode {
    sno: u32,
    ino: u32,
    superblock: Arc<Ext4>,
    dents_cache: Option<Vec<DirResult>>,
}

impl Ext4Inode {
    pub fn new(sno: u32, ino: u32, superblock: &Arc<Ext4>) -> Self {
        Self {
            sno,
            ino,
            superblock: superblock.clone(),
            dents_cache: None,
        }
    }
}

impl Inode for Ext4Inode {
    fn get_ino(&self) -> u32 {
        self.ino
    }

    fn get_sno(&self) -> u32 {
        self.sno
    }

    fn type_name(&self) -> &'static str {
        "ext4"
    }

    fn create(&mut self, name: &str, mode: Mode) -> SysResult<()> {
        self.superblock.create(self.ino, name, mode.bits()).unwrap();
        self.dents_cache = None; // Invalidate cache
        Ok(())
    }

    fn unlink(&mut self, _name: &str) -> SysResult<()> {
        Err(Errno::EOPNOTSUPP)
    }

    fn readat(&mut self, buf: &mut [u8], offset: usize) -> SysResult<usize> {
        self.superblock.read_at(self.ino, offset, buf).map_err(|_| Errno::EIO)
    }

    fn writeat(&mut self, buf: &[u8], offset: usize) -> SysResult<usize> {
        self.superblock.write_at(self.ino, offset, buf).map_err(|_| Errno::EIO)
    }

    fn get_dent(&mut self, index: usize) -> SysResult<Option<DirResult>> {
        if let Some(dents) = &self.dents_cache {
            if index < dents.len() {
                Ok(Some(dents[index].clone()))
            } else {
                Ok(None)
            }
        } else {
            let mut dents = Vec::new();

            self.superblock.dir_get_entries(self.ino).iter().for_each(|entry| {
                if entry.inode != 0 {
                    dents.push(DirResult {
                        name: entry.get_name(),
                        ino: entry.inode as u32,
                        file_type: match self.superblock.get_inode_ref(entry.inode).inode.file_type() {
                            InodeFileType::S_IFBLK => FileType::BlockDevice,
                            InodeFileType::S_IFCHR => FileType::CharDevice,
                            InodeFileType::S_IFDIR => FileType::Directory,
                            InodeFileType::S_IFIFO => FileType::FIFO,
                            InodeFileType::S_IFLNK => FileType::Symlink,
                            InodeFileType::S_IFREG => FileType::Regular,
                            InodeFileType::S_IFSOCK => FileType::Socket,
                            _ => FileType::Unknown,
                        }
                    });
                }
            });

            let r = if index >= dents.len() {
                None
            } else {
                let r = dents[index].clone();
                Some(r)
            };

            self.dents_cache = Some(dents);

            Ok(r)
        }
    }

    fn lookup(&mut self, name: &str) -> SysResult<u32> {
        let mut result = Ext4DirSearchResult::new(Ext4DirEntry::default());
        self.superblock.dir_find_entry(self.ino, name, &mut result).map_err(error_map)?;
        Ok(result.dentry.inode as u32)
    }

    fn size(&self) -> Result<usize, Errno> {
        // ffi::inode_get_size(self.inode_handler)
        Ok(self.superblock.get_inode_ref(self.ino).inode.size() as usize)
    }

    fn mode(&self) -> Mode {
        Mode::from_bits_truncate(self.superblock.get_inode_ref(self.ino).inode.mode())
    }
}

impl Drop for Ext4Inode {
    fn drop(&mut self) {
        
    }
}
