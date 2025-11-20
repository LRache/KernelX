use core::{mem, slice};

use crate::{Ext4Result, SystemHal, error::Context, ffi::*, util::revision_tuple};

use super::{InodeRef, InodeType};

impl<Hal: SystemHal> InodeRef<Hal> {
    pub fn read_dir(mut self, offset: u64) -> Ext4Result<DirReader<Hal>> {
        unsafe {
            let mut iter = mem::zeroed();
            ext4_dir_iterator_init(&mut iter, self.inner.as_mut(), offset)
                .context("ext4_dir_iterator_init")?;

            Ok(DirReader {
                parent: self,
                inner: iter,
            })
        }
    }

    pub fn lookup(mut self, name: &str) -> Ext4Result<DirLookupResult<Hal>> {
        unsafe {
            let mut result = mem::zeroed();
            ext4_dir_find_entry(
                &mut result,
                self.inner.as_mut(),
                name.as_ptr() as *const _,
                name.len() as _,
            )
            .context("ext4_dir_find_entry")?;

            Ok(DirLookupResult {
                parent: self,
                inner: result,
            })
        }
    }

    pub fn has_children(self) -> Ext4Result<bool> {
        if self.inode_type() != InodeType::Directory {
            return Ok(false);
        }
        let mut reader = self.read_dir(0)?;
        while let Some(curr) = reader.current() {
            let name = curr.name();
            if name != b"." && name != b".." {
                return Ok(true);
            }
            reader.step()?;
        }
        Ok(false)
    }

    pub(crate) fn add_entry(&mut self, name: &str, entry: &mut InodeRef<Hal>) -> Ext4Result {
        unsafe {
            ext4_dir_add_entry(
                self.inner.as_mut(),
                name.as_ptr() as *const _,
                name.len() as _,
                entry.inner.as_mut(),
            )
            .context("ext4_dir_add_entry")?;
        }
        entry.inc_nlink();
        Ok(())
    }
    pub(crate) fn remove_entry(&mut self, name: &str, entry: &mut InodeRef<Hal>) -> Ext4Result {
        unsafe {
            ext4_dir_remove_entry(
                self.inner.as_mut(),
                name.as_ptr() as *const _,
                name.len() as _,
            )
            .context("ext4_dir_remove_entry")?;
        }
        entry.dec_nlink();
        Ok(())
    }
}

pub struct DirLookupResult<Hal: SystemHal> {
    parent: InodeRef<Hal>,
    inner: ext4_dir_search_result,
}
impl<Hal: SystemHal> DirLookupResult<Hal> {
    pub fn entry(&mut self) -> DirEntry {
        DirEntry {
            inner: unsafe { &mut *(self.inner.dentry as *mut _) },
            sb: self.parent.superblock(),
        }
    }
}
impl<Hal: SystemHal> Drop for DirLookupResult<Hal> {
    fn drop(&mut self) {
        unsafe {
            ext4_dir_destroy_result(self.parent.inner.as_mut(), &mut self.inner);
        }
    }
}

#[repr(transparent)]
pub struct RawDirEntry {
    inner: ext4_dir_en,
}
impl RawDirEntry {
    pub fn ino(&self) -> u32 {
        u32::from_le(self.inner.inode)
    }
    pub fn set_ino(&mut self, ino: u32) {
        self.inner.inode = u32::to_le(ino);
    }

    pub fn len(&self) -> u16 {
        u16::from_le(self.inner.entry_len)
    }

    pub fn name<'a>(&'a self, sb: &ext4_sblock) -> &'a [u8] {
        let mut name_len = self.inner.name_len as u16;
        if revision_tuple(sb) < (0, 5) {
            let high = unsafe { self.inner.in_.name_length_high };
            name_len |= (high as u16) << 8;
        }
        unsafe { slice::from_raw_parts(self.inner.name.as_ptr(), name_len as usize) }
    }

    pub fn inode_type(&self, sb: &ext4_sblock) -> InodeType {
        if revision_tuple(sb) < (0, 5) {
            InodeType::Unknown
        } else {
            match unsafe { self.inner.in_.inode_type } as u32 {
                EXT4_DE_DIR => InodeType::Directory,
                EXT4_DE_REG_FILE => InodeType::RegularFile,
                EXT4_DE_SYMLINK => InodeType::Symlink,
                EXT4_DE_CHRDEV => InodeType::CharacterDevice,
                EXT4_DE_BLKDEV => InodeType::BlockDevice,
                EXT4_DE_FIFO => InodeType::Fifo,
                EXT4_DE_SOCK => InodeType::Socket,
                _ => InodeType::Unknown,
            }
        }
    }
}

pub struct DirEntry<'a> {
    inner: &'a mut RawDirEntry,
    sb: &'a ext4_sblock,
}
impl DirEntry<'_> {
    pub fn ino(&self) -> u32 {
        self.inner.ino()
    }

    pub fn name(&self) -> &[u8] {
        self.inner.name(self.sb)
    }

    pub fn inode_type(&self) -> InodeType {
        self.inner.inode_type(self.sb)
    }

    pub fn len(&self) -> u16 {
        self.inner.len()
    }

    pub fn is_empty(&self) -> bool {
        self.inner.len() == 0
    }

    pub fn raw_entry(&self) -> &RawDirEntry {
        self.inner
    }
    pub fn raw_entry_mut(&mut self) -> &mut RawDirEntry {
        self.inner
    }
}

/// Reader returned by [`InodeRef::read_dir`].
pub struct DirReader<Hal: SystemHal> {
    parent: InodeRef<Hal>,
    inner: ext4_dir_iter,
}
impl<Hal: SystemHal> DirReader<Hal> {
    pub fn current(&self) -> Option<DirEntry> {
        if self.inner.curr.is_null() {
            return None;
        }
        let curr = unsafe { &mut *(self.inner.curr as *mut _) };
        let sb = self.parent.superblock();

        Some(DirEntry { inner: curr, sb })
    }

    pub fn step(&mut self) -> Ext4Result {
        if !self.inner.curr.is_null() {
            unsafe {
                ext4_dir_iterator_next(&mut self.inner).context("ext4_dir_iterator_next")?;
            }
        }
        Ok(())
    }

    pub fn offset(&self) -> u64 {
        self.inner.curr_off
    }
}
impl<Hal: SystemHal> Drop for DirReader<Hal> {
    fn drop(&mut self) {
        unsafe {
            ext4_dir_iterator_fini(&mut self.inner);
        }
    }
}
