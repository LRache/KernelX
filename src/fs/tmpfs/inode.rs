use alloc::sync::Arc;
use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::vec::Vec;

use crate::kernel::scheduler::current::copy_from_user::buffer;
use crate::{arch, kinfo};
use crate::kernel::errno::{SysResult, Errno};
use crate::fs::InodeOps;
use crate::fs::inode::Mode;
use crate::kernel::mm::PhysPageFrame;
use crate::klib::SpinLock;

use super::superblock::SuperBlockInner;

enum Meta {
    File { pages: Vec<PhysPageFrame>, filesize: usize },
    Directory(BTreeMap<String, u32>)
}

pub struct InodeMeta {
    meta: Meta,
    filesz: usize,
    mode: Mode,
}

impl InodeMeta {
    pub fn new(mode: Mode) -> Self {
        let meta = if mode.contains(Mode::S_IFDIR) {
            Meta::Directory(BTreeMap::new())
        } else {
            Meta::File { pages: Vec::new(), filesize: 0 }
        };
        Self { 
            meta, 
            filesz: 0,
            mode
        }
    }
}

pub struct Inode {
    ino: u32,
    sno: u32,
    meta: Arc<SpinLock<InodeMeta>>,
    superblock: Arc<SpinLock<SuperBlockInner>>,
}

impl Inode {
    pub fn new(ino: u32, sno: u32, meta: Arc<SpinLock<InodeMeta>>, superblock: Arc<SpinLock<SuperBlockInner>>) -> Self {
        Self {
            ino,
            sno,
            meta,
            superblock,
        }

    }
}

impl InodeOps for Inode {
    fn create(&self, name: &str, mode: Mode) -> SysResult<()> {
        if let Meta::Directory(ref mut children) = self.meta.lock().meta {
            if children.contains_key(name) {
                return Err(Errno::EEXIST);
            }

            let ino = self.superblock.lock().alloc_inode(mode);
            children.insert(name.into(), ino);

            Ok(())
        } else {
            Err(Errno::ENOTDIR)
        }
    }

    fn get_ino(&self) -> u32 {
        self.ino
    }

    fn get_sno(&self) -> u32 {
        self.sno
    }

    fn lookup(&self, name: &str) -> SysResult<u32> {
        if let Meta::Directory(ref children) = self.meta.lock().meta {
            if let Some(&ino) = children.get(name) {
                Ok(ino)
            } else {
                Err(Errno::ENOENT)
            }
        } else {
            Err(Errno::ENOTDIR)
        }
    }

    fn readat(&self, buf: &mut [u8], offset: usize) -> SysResult<usize> {
        if let Meta::File { ref pages, ref filesize } = self.meta.lock().meta {
            if offset >= *filesize {
                return Ok(0);
            }
            
            let mut total_read = 0;
            let mut current_offset = offset;
            let mut left = core::cmp::min(buf.len(), *filesize - offset);

            while left > 0 {
                let page_index = current_offset / arch::PGSIZE;
                let page_offset = current_offset % arch::PGSIZE;

                if page_index >= pages.len() {
                    break;
                }

                let page = &pages[page_index];
                let to_read = core::cmp::min(left, 4096 - page_offset);

                page.copy_to_slice(page_offset, &mut buf[total_read..total_read+to_read]);

                left -= to_read;
                total_read += to_read;
                current_offset += to_read;
            }

            Ok(total_read)
        } else {
            Err(Errno::EISDIR)
        }
    }

    fn writeat(&self, buf: &[u8], offset: usize) -> Result<usize, Errno> {
        if let Meta::File { ref mut pages, ref mut filesize } = self.meta.lock().meta {
            let mut written_bytes = 0;
            let mut current_offset = offset;

            while written_bytes < buf.len() {
                let page_index = current_offset / arch::PGSIZE;
                let page_offset = current_offset % arch::PGSIZE;

                while page_index >= pages.len() {
                    pages.push(PhysPageFrame::alloc());
                }

                let page = &pages[page_index];
                let to_write = core::cmp::min(buf.len() - written_bytes, 4096 - page_offset);

                page.copy_from_slice(page_offset, &buf[written_bytes..written_bytes+to_write]);

                written_bytes += to_write;
                current_offset += to_write;
            }

            *filesize = core::cmp::max(*filesize, offset + written_bytes);

            Ok(written_bytes)
        } else {
            Err(Errno::EISDIR)
        }
    }

    fn size(&self) -> SysResult<u64> {
        Ok(self.meta.lock().filesz as u64)
    }

    fn type_name(&self) -> &'static str {
        "tmp"
    }
}
