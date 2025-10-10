use alloc::string::String;
use alloc::sync::Arc;
use alloc::vec::Vec;
use ext4_rs::Ext4InodeRef;
use ext4_rs::Ext4Inode as Meta;
use spin::Mutex;

use crate::kernel::errno::{Errno, SysResult};
use crate::fs::vfs;
use crate::fs::inode::{Inode, Mode};
use crate::fs::file::DirResult;

use super::superblock_inner::{SuperBlockInnerExt, SuperBlockInner, map_error};

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

    fn create(&self, name: &str, mode: Mode) -> SysResult<()> {
        *self.dents_cache.lock() = None; // Invalidate cache

        self.with_inode_ref(|inode_ref| {
            self.superblock.create_ref(inode_ref, name, mode.bits()).map_err(|_| Errno::EIO)
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
        self.with_inode_ref(|inode_ref| {
            self.superblock.readat_ref(inode_ref, offset, buf).map_err(|_| Errno::EIO)
        })
    }

    fn writeat(&self, buf: &[u8], offset: usize) -> SysResult<usize> {
        self.with_inode_ref(|inode_ref| {
            self.superblock.writeat_ref(inode_ref, offset, buf).map_err(|_| Errno::EIO)
        })
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
                            name: String::from_utf8_lossy(&entry.name).into_owned(),
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
    }

    fn lookup(&self, name: &str) -> SysResult<u32> {
        let r = self.with_inode_ref(|inode_ref| {
            self.superblock.dir_find_entry_ref(inode_ref, name)
        })?.ok_or(Errno::ENOENT)?;
        
        Ok(r.dentry.inode as u32)
    }

    fn rename(&self, old_name: &str, new_parent: &Arc<dyn Inode>, new_name: &str) -> SysResult<()> {
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
}

impl Drop for Ext4Inode {
    fn drop(&mut self) {
        let meta = self.meta.lock();
        if meta.links_count() == 0 {
            self.superblock.ialloc_free_inode(self.ino, meta.is_dir());
        }
    }
}
