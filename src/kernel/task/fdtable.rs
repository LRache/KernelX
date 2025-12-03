use alloc::sync::Arc;
use alloc::vec::Vec;
use alloc::vec;

use crate::fs::file::FileOps;
use crate::kernel::config;
use crate::kernel::errno::{Errno, SysResult};

#[derive(Clone, Copy)]
pub struct FDFlags {
    pub cloexec: bool,
}

impl FDFlags {
    pub fn empty() -> Self {
        Self { cloexec: false }
    }
}

#[derive(Clone)]
struct FDItem {
    pub file: Arc<dyn FileOps>,
    pub flags: FDFlags,
}

pub struct FDTable {
    table: Vec<Option<FDItem>>,
    max_fd: usize,
}

impl FDTable {
    pub fn new() -> Self {
        Self {
            table: vec![None; 32], // Initialize with 32 file descriptors
            max_fd: config::MAX_FD,
        }
    }

    pub fn get(&mut self, fd: usize) -> SysResult<Arc<dyn FileOps>> {
        if fd < self.table.len() {
            let item = self.table[fd].as_ref().ok_or(Errno::EBADF)?;
            Ok(item.file.clone())
        } else {
            Err(Errno::EBADF)
        }
    }

    pub fn set(&mut self, fd: usize, file: Arc<dyn FileOps>, flags: FDFlags) -> SysResult<()> {
        if fd >= config::MAX_FD {
            return Err(Errno::EBADF);
        }
        if fd >= self.table.len() {
            self.table.resize(fd + 1, None);
        }
        self.table[fd] = Some(FDItem { file, flags });
        Ok(())
    }

    pub fn get_fd_flags(&self, fd: usize) -> SysResult<FDFlags> {
        if fd < self.table.len() {
            let item = self.table[fd].as_ref().ok_or(Errno::EBADF)?;
            Ok(item.flags)
        } else {
            Err(Errno::EBADF)
        }
    }

    pub fn set_fd_flags(&mut self, fd: usize, flags: FDFlags) -> SysResult<()> {
        if fd < self.table.len() {
            let item = self.table[fd].as_mut().ok_or(Errno::EBADF)?;
            item.flags = flags;
            Ok(())
        } else {
            Err(Errno::EBADF)
        }
    }

    pub fn push(&mut self, file: Arc<dyn FileOps>, flags: FDFlags) -> Result<usize, Errno> {
        if let Some(pos) = self.table.iter().position(|f| f.is_none()) {
            self.table[pos] = Some(FDItem { file, flags });
            Ok(pos)
        } else {
            if self.table.len() >= self.max_fd {
                return Err(Errno::EMFILE);
            }
            self.table.push(Some(FDItem { file, flags }));
            Ok(self.table.len() - 1)
        }
    }

    pub fn close(&mut self, fd: usize) -> SysResult<()> {
        if fd < self.table.len() {
            if self.table[fd].is_none() {
                return Err(Errno::EBADF);
            }
            self.table[fd] = None;
            Ok(())
        } else {
            Err(Errno::EBADF)
        }
    }

    pub fn fork(&self) -> Self {
        let new_table = self.table.iter().map(|item| {
            item.as_ref().map(|fd_item| fd_item.clone())
        }).collect();
        
        Self { table: new_table, max_fd: self.max_fd }
    }

    pub fn cloexec(&mut self) {
        self.table.iter_mut().for_each(|item| {
            if let Some(fd_item) = item {
                if fd_item.flags.cloexec {
                    *item = None;
                }
            }
        });
    }

    pub fn set_max_fd(&mut self, max_fd: usize) {
        self.max_fd = max_fd;
    }

    pub fn get_max_fd(&self) -> usize {
        self.max_fd
    }
}
