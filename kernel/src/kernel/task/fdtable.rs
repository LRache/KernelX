use alloc::sync::Arc;
use alloc::vec::{Vec};
use alloc::vec;
use spin::Mutex;

use crate::fs::file::File;
use crate::kernel::errno::Errno;

pub struct FDTable {
    pub table: Mutex<Vec<Option<Arc<File>>>>,
}

impl FDTable {
    pub fn new() -> Self {
        Self {
            table: Mutex::new(vec![None; 32]), // Initialize with 32 file descriptors
        }
    }

    pub fn get(&self, fd: usize) -> Result<Arc<File>, Errno> {
        let table = self.table.lock();
        if fd < table.len() {
            table[fd].clone().ok_or(Errno::EBADFD)
        } else {
            Err(Errno::EBADFD)
        }
    }

    pub fn push(&self, file: Arc<File>) -> Result<usize, Errno> {
        let mut table = self.table.lock();
        if let Some(pos) = table.iter().position(|f| f.is_none()) {
            table[pos] = Some(file);
            Ok(pos)
        } else {
            table.push(Some(file));
            Ok(table.len() - 1)
        }
    }

    pub fn close(&self, fd: usize) -> Result<(), Errno> {
        let mut table = self.table.lock();
        if fd < table.len() {
            if let Some(file) = table[fd].take() {
                file.close()?;
            }
            Ok(())
        } else {
            Err(Errno::EBADFD)
        }
    }

    pub fn fork(&self) -> Self {
        let table = self.table.lock();
        
        let mut new_table = vec![None; table.len()];
        for (i, file) in table.iter().enumerate() {
            if let Some(file) = file {
                new_table[i] = Some(file.clone());
            }
        }
        
        Self {
            table: Mutex::new(new_table),
        }
    }
}
