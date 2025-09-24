use alloc::sync::Arc;
use alloc::vec::Vec;
use ext4_rs::{Ext4, Ext4DirEntry, Ext4DirSearchResult, InodeFileType};

use crate::kernel::errno::{Errno, SysResult};
use crate::fs::inode::{Inode, Mode};
use crate::fs::file::{DirResult, FileType};
use crate::ktrace;

pub struct Ext4Inode {
    sno: u32,
    ino: u32,
    superblock: Arc<Ext4>,
    dents_cache: Option<Vec<DirResult>>,
    // pub ino: u32,
    // sno: u32,
    // inode_handler: usize,
    // destroyed: bool
}

impl Ext4Inode {
    // pub fn new(ino: u32, sno: u32, fs_handler: usize) -> Result<Self, Errno> {
    //     Ok(Self {
    //         ino,
    //         sno,
    //         inode_handler: ffi::get_inode_handler(fs_handler, ino)?,
    //         destroyed: false
    //     })
    // }
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
        // ffi::create_inode(self.inode_handler, name, 0644).map(|_| ())
        self.superblock.create(self.ino, name, mode.bits()).unwrap();
        self.dents_cache = None; // Invalidate cache
        Ok(())
    }

    fn readat(&mut self, buf: &mut [u8], offset: usize) -> Result<usize, Errno> {
        // Ok(ffi::inode_readat(self.inode_handler, buf, offset)? as usize)
        // ktrace!("readat ino={} offset={} size={}", self.ino, offset, buf.len());
        self.superblock.read_at(self.ino, offset, buf).map_err(|_| Errno::EIO)
    }

    fn writeat(&mut self, buf: &[u8], offset: usize) -> Result<usize, Errno> {
        // ffi::inode_writeat(self.inode_handler, buf, offset)
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
        // ffi::inode_lookup(self.inode_handler, name)
        let mut result = Ext4DirSearchResult::new(Ext4DirEntry::default());
        // kinfo!("lookup {} in inode {}", name, self.ino);
        self.superblock.dir_find_entry(self.ino, name, &mut result).map_err(|_| Errno::EIO)?;
        ktrace!("found inode {} for name {}", result.dentry.inode, name);
        Ok(result.dentry.inode as u32)
    }

    // fn mkdir(&mut self, name: &str) -> SysResult<()> {
    //     // ffi::inode_mkdir(self.inode_handler, name).map(|_| ())
    //     self.superblock.dir_mk()
    // }

    fn size(&self) -> Result<usize, Errno> {
        // ffi::inode_get_size(self.inode_handler)
        Ok(self.superblock.get_inode_ref(self.ino).inode.size() as usize)
    }

    fn mode(&self) -> Mode {
        Mode::from_bits_truncate(self.superblock.get_inode_ref(self.ino).inode.mode())
    }

    // fn destroy(&mut self) -> SysResult<()> {
    //     if !self.destroyed {
    //         ffi::put_inode_handler(self.inode_handler)?;
    //         self.destroyed = true;
    //     }
    //     Ok(())
    // }
}

impl Drop for Ext4Inode {
    // fn drop(&mut self) {
    //     if !self.destroyed {
    //         let _ = self.destroy();
    //         self.destroyed = true;
    //     }
    // }
    fn drop(&mut self) {
        
    }
}
