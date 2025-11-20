use alloc::sync::Arc;
use alloc::vec::Vec;
use ext4_rs::{Ext4InodeRef, Ext4DirEntry, Ext4DirSearchResult};
use ext4_rs::Ext4Inode as Meta;
use lwext4_rust::
use spin::Mutex;

use crate::kernel::errno::{Errno, SysResult};
use crate::kernel::uapi::FileStat;
use crate::fs::vfs;
use crate::fs::inode::{InodeOps, Mode};
use crate::fs::file::DirResult;

use super::superblock_inner::{SuperBlockInner, SuperBlockInnerExt, map_error};

pub struct Ext4Inode {
    ino: u32,
    sno: u32,
    meta: Mutex<Meta>,
    superblock: Arc<SuperBlockInner>,
    dents_cache: Mutex<Option<Vec<DirResult>>>,
}

impl Ext4Inode {
    pub fn new(sno: u32, ino: u32, superblock: &Arc<SuperBlockInner>) -> Self {
        let inode_ref = superblock.get_inode_ref(ino);
        Self {
            ino,
            sno,
            meta: Mutex::new(inode_ref.inode),
            superblock: superblock.clone(),
            dents_cache: Mutex::new(None),
        }
    }

    fn with_inode_ref<F, R>(&self, f: F) -> R
    where
        F: FnOnce(&mut Ext4InodeRef) -> R,
    {
        let mut meta = self.meta.lock();
        let mut inode_ref = Ext4InodeRef {
            inode: *meta,
            inode_num: self.ino,
        };
        let r = f(&mut inode_ref);
        *meta = inode_ref.inode;
        r
    }

    fn get_inode_ref(&self, meta: &Meta) -> Ext4InodeRef {
        Ext4InodeRef {
            inode: *meta,
            inode_num: self.ino,
        }
    }
}

impl InodeOps for Ext4Inode {
    fn get_ino(&self) -> u32 {
        self.ino
    }

    fn get_sno(&self) -> u32 {
        self.sno
    }

    fn type_name(&self) -> &'static str {
        "ext4"
    }

    fn create(&self, name: &str, mode: Mode) -> SysResult<()> {
        if !self.mode().contains(Mode::S_IFDIR) {
            return Err(Errno::ENOTDIR);
        }
        
        *self.dents_cache.lock() = None; // Invalidate cache

        self.with_inode_ref(|inode_ref| {
            self.superblock.create_ref(inode_ref, name, mode.bits())
        })?;
        
        Ok(())
    }

    fn unlink(&self, name: &str) -> SysResult<()> {
        if name == "." || name == ".." {
            return Err(Errno::EINVAL);
        }
            
        *self.dents_cache.lock() = None; // Invalidate cache

        let child = vfs::load_inode(self.sno, self.lookup(name)?)?.downcast_arc::<Ext4Inode>().map_err(|_| "TypeError").unwrap();
        
        {
            let mut child_meta = child.meta.lock();
            let nlink = child_meta.links_count();
            assert!(nlink > 0);
            child_meta.set_links_count(nlink - 1);
        }
        
        let _ = child.sync();

        self.with_inode_ref(|inode_ref| {
            self.superblock.dir_remove_entry(inode_ref, name)
        }).map_err(map_error)?;

        Ok(())
    }

    fn readat(&self, buf: &mut [u8], offset: usize) -> SysResult<usize> {
        if self.mode().contains(Mode::S_IFDIR) {
            return Err(Errno::EISDIR);
        }
        // self.with_inode_ref(|inode_ref| {
        //     self.superblock.readat_ref(inode_ref, offset, buf).map_err(|_| Errno::EIO)
        // })
        self.superblock.read_at(self.ino, offset, buf).map_err(map_error)
    }

    fn writeat(&self, buf: &[u8], offset: usize) -> SysResult<usize> {
        if self.mode().contains(Mode::S_IFDIR) {
            return Err(Errno::EISDIR);
        }
        self.with_inode_ref(|inode_ref| {
            // self.superblock.write_back_inode(inode_ref);
            // let mut inode_ref = self.superblock.get_inode_ref(self.ino);
            self.superblock.writeat_ref(inode_ref, offset, buf)
            // self.superblock.write_back_inode(inode_ref);
            // *inode_ref = self.superblock.get_inode_ref(self.ino);
            // Ok(r)
        })
        // kinfo!("Ext4Inode::writeat ino={} offset={} len={}", self.ino, offset, buf.len());
        // self.superblock.get_inode_ref(self.ino);
        // self.superblock.write_at(self.ino, offset, buf).map_err(map_error)
    }

    fn get_dent(&self, index: usize) -> SysResult<Option<DirResult>> {
        if let Some(dents) = &self.dents_cache.lock().as_ref() {
            if index < dents.len() {
                Ok(Some(dents[index].clone()))
            } else {
                Ok(None)
            }
        } else {
            let mut dents = Vec::new();

            self.with_inode_ref(|inode_ref| {
                self.superblock.dir_get_entries_ref(inode_ref)
            })?.iter().for_each(|entry| {
                if entry.inode != 0 {
                    // To avoid Deadlock
                    if entry.inode == self.ino {
                        dents.push(DirResult {
                            name: entry.get_name(),
                            ino: entry.inode as u32,
                            file_type: self.inode_type(),
                        });
                    } else {
                        let child = vfs::load_inode(self.sno, entry.inode).unwrap();
                        let file_type = child.inode_type();
                        dents.push(DirResult {
                            name: entry.get_name(),
                            ino: entry.inode as u32,
                            file_type
                        });
                    }
                }
            });

            let r = if index >= dents.len() {
                None
            } else {
                let r = dents[index].clone();
                Some(r)
            };

            *self.dents_cache.lock() = Some(dents);

            Ok(r)
        }
        // unimplemented!();
    }

    fn lookup(&self, name: &str) -> SysResult<u32> {
        // let r = self.with_inode_ref(|inode_ref| {
        //     self.superblock.dir_find_entry_ref(inode_ref, name)
        // })?.ok_or(Errno::ENOENT)?;
        // let r = self.superblock.dir_find_entry(self.ino, name)?.ok_or(Errno::ENOENT)?;
        
        // Ok(r.dentry.inode as u32)
        let mut result = Ext4DirSearchResult::new(Ext4DirEntry::default());
        // kinfo!("lookup: {}", name);
        self.superblock.dir_find_entry(self.ino, name, &mut result).map_err(map_error)?;
        Ok(result.dentry.inode)
    }

    fn rename(&self, old_name: &str, new_parent: &Arc<dyn InodeOps>, new_name: &str) -> SysResult<()> {
        let new_parent_inode = new_parent.clone().downcast_arc::<Ext4Inode>().map_err(|_| "TypeError").unwrap();

        let mut meta = self.meta.lock();

        let mut self_inode_ref = self.get_inode_ref(&meta);
        let child_ino = self.superblock.dir_find_entry_ref(&mut self_inode_ref, old_name)?.ok_or(Errno::ENOENT)?.dentry.inode;
        *meta = self_inode_ref.inode;

        let child_inode = vfs::load_inode(self.sno, child_ino as u32)?.downcast_arc::<Ext4Inode>().map_err(|_| "TypeError").unwrap();
        
        assert!(!Arc::ptr_eq(&new_parent_inode, &child_inode));

        let mut helper = |new_parent_ref: &mut Ext4InodeRef| -> SysResult<()> {
            let r = self.superblock.dir_find_entry_ref(new_parent_ref, new_name)?;
            if r.is_some() {
                self.superblock.dir_remove_entry(new_parent_ref, old_name).map_err(map_error)?;
            }

            child_inode.with_inode_ref(|child_ref| {
                self.superblock.link(&mut self_inode_ref, child_ref, new_name)
            }).map_err(map_error)?;

            Ok(())
        };

        if self.ino == new_parent.get_ino() {
            let mut self_inode_ref = self.get_inode_ref(&meta);
            helper(&mut self_inode_ref)?;
            *meta = self_inode_ref.inode;
        } else {
            helper(&mut new_parent_inode.get_inode_ref(&new_parent_inode.meta.lock()))?;
        }

        let mut self_inode_ref = self.get_inode_ref(&meta);
        self.superblock.dir_remove_entry(&mut self_inode_ref, old_name).map_err(map_error)?;
        *meta = self_inode_ref.inode;

        Ok(())
    }

    fn size(&self) -> SysResult<u64> {
        Ok(self.meta.lock().size())
    }

    fn mode(&self) -> Mode {
        Mode::from_bits_truncate(self.meta.lock().mode())
    }

    fn fstat(&self) -> SysResult<FileStat> {
        let mut kstat = FileStat::default();
        let meta = self.meta.lock();

        kstat.st_ino = self.ino as u64;
        kstat.st_size = meta.size() as i64;
        kstat.st_nlink = meta.links_count() as u32;
        kstat.st_mode = meta.mode() as u32;
        // TODO: utime tests
        // kstat.st_atime_sec = meta.atime() as i64;
        kstat.st_atime_sec = 0;
        kstat.st_atime_nsec = 0;
        // kstat.st_mtime_sec = meta.mtime() as i64;
        kstat.st_mtime_sec = 0;
        kstat.st_mtime_nsec = 0;
        // kstat.st_ctime_sec = meta.ctime() as i64;
        kstat.st_ctime_sec = 0;
        kstat.st_ctime_nsec = 0;

        Ok(kstat)
    }

    fn truncate(&self, new_size: u64) -> SysResult<()> {
        self.with_inode_ref(|inode_ref| {
            if Mode::from_bits_truncate(inode_ref.inode.mode()).contains(Mode::S_IFDIR) {
                return Err(Errno::EISDIR);
            }

            let old_size = inode_ref.inode.size();
            match old_size.cmp(&new_size) {
                core::cmp::Ordering::Equal => {},
                core::cmp::Ordering::Less => {
                    self.superblock.writeat_ref(inode_ref, new_size as usize, &[0; 1])?;
                },
                core::cmp::Ordering::Greater => {
                    self.superblock.truncate_inode(inode_ref, new_size).map_err(map_error)?;
                },
            }
            Ok(())
        })
    }

    fn update_atime(&self, atime: u64, atime_nsec: u64) -> SysResult<()> {
        let mut meta = self.meta.lock();
        meta.set_atime(atime as u32);
        let _ = atime_nsec;
        Ok(())
    }

    fn update_mtime(&self, mtime: u64, mtime_nsec: u64) -> SysResult<()> {
        let mut meta = self.meta.lock();
        meta.set_mtime(mtime as u32);
        let _ = mtime_nsec;
        Ok(())
    }

    fn update_ctime(&self, ctime: u64, ctime_nsec: u64) -> SysResult<()> {
        let mut meta = self.meta.lock();
        meta.set_ctime(ctime as u32);
        let _ = ctime_nsec;
        Ok(())
    }

    fn sync(&self) -> SysResult<()> {
        self.superblock.write_back_inode(&mut self.get_inode_ref(&*self.meta.lock()));
        Ok(())
    }
}

impl Drop for Ext4Inode {
    fn drop(&mut self) {
        let meta = self.meta.lock();
        self.superblock.write_back_inode(&mut self.get_inode_ref(&*meta));
        if meta.links_count() == 0 {
            self.superblock.ialloc_free_inode(self.ino, meta.is_dir());
        }
    }
}
