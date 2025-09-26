use alloc::sync::Arc;
use alloc::vec::Vec;
use alloc::vec;

use crate::fs::file::{File, FileOps};
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
    pub file: Arc<File>,
    pub flags: FDFlags,
}

pub struct FDTable {
    table: Vec<Option<FDItem>>,
}

impl FDTable {
    pub fn new() -> Self {
        Self {
            table: vec![None; 32], // Initialize with 32 file descriptors
        }
    }

    pub fn get(&mut self, fd: usize) -> SysResult<Arc<dyn FileOps>> {
        if fd < self.table.len() {
            let item = self.table[fd].as_ref().ok_or(Errno::EBADFD)?;
            Ok(item.file.clone())
        } else {
            Err(Errno::EBADFD)
        }
    }

    pub fn set(&mut self, fd: usize, file: Arc<File>, flags: FDFlags) -> SysResult<()> {
        if fd >= config::MAX_FD {
            return Err(Errno::EBADFD);
        }
        if fd >= self.table.len() {
            self.table.resize(fd + 1, None);
        }
        self.table[fd] = Some(FDItem { file, flags });
        Ok(())
    }

    pub fn push(&mut self, file: Arc<File>, flags: FDFlags) -> Result<usize, Errno> {
        if let Some(pos) = self.table.iter().position(|f| f.is_none()) {
            self.table[pos] = Some(FDItem { file, flags });
            Ok(pos)
        } else {
            self.table.push(Some(FDItem { file, flags }));
            Ok(self.table.len() - 1)
        }
    }

    pub fn close(&mut self, fd: usize) -> Result<(), Errno> {
        if fd < self.table.len() {
            if self.table[fd].is_none() {
                return Err(Errno::EBADFD);
            }
            self.table[fd] = None;
            Ok(())
        } else {
            Err(Errno::EBADFD)
        }
    }

    pub fn fork(&self) -> Self {
        let mut new_table = vec![None; self.table.len()];
        for (i, file) in self.table.iter().enumerate() {
            if let Some(file) = file {
                new_table[i] = Some(file.clone());
            }
        }
        
        Self {
            table: new_table,
        }
    }

    pub fn cloexec(&mut self) {
        for item in self.table.iter_mut() {
            if let Some(fd_item) = item {
                if fd_item.flags.cloexec {
                    *item = None;
                }
            }
        }
    }
}
