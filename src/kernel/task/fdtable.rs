use alloc::sync::Arc;
use alloc::vec;
use alloc::vec::Vec;

use crate::fs::file::FileOps;
use crate::kernel::config;
use crate::kernel::errno::{Errno, SysResult};
use crate::kernel::scheduler::{Tid, current};

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
    owner: Tid,
}

fn release_posix_locks_from_file(file: &Arc<dyn FileOps>, owner: Tid) {
    let Some(inode) = file.get_inode() else {
        return;
    };

    if let Some(lock_state) = inode.lock_state() {
        let mut lock_state = lock_state.lock();
        if lock_state.posix.remove_owner(owner) {
            lock_state.posix.wake_all();
        }
    }
}

impl FDTable {
    pub fn new(owner: Tid) -> Self {
        Self {
            table: vec![None; 32], // Initialize with 32 file descriptors
            max_fd: config::MAX_FD,
            owner,
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
        if let Some(old_item) = self.table[fd].as_ref()
            && !Arc::ptr_eq(&old_item.file, &file)
        {
            if current::has_task() {
                release_posix_locks_from_file(&old_item.file, current::pid());
            }
            old_item.file.on_fd_remove();
        }
        if self.table[fd].is_none() {
            file.on_fd_install();
        } else if let Some(old_item) = self.table[fd].as_ref()
            && !Arc::ptr_eq(&old_item.file, &file)
        {
            file.on_fd_install();
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
            self.set(pos, file, flags)?;
            Ok(pos)
        } else {
            if self.table.len() >= self.max_fd {
                return Err(Errno::EMFILE);
            }
            let pos = self.table.len();
            self.set(pos, file, flags)?;
            Ok(pos)
        }
    }

    pub fn dup_min(&mut self, fd: usize, min_fd: usize, flags: FDFlags) -> SysResult<usize> {
        let file = self.get(fd)?;
        if let Some(new_fd) = self
            .table
            .iter()
            .skip(min_fd)
            .position(|f| f.is_none())
            .map(|p| p + min_fd)
        {
            self.set(new_fd, file, flags)?;
            Ok(new_fd)
        } else {
            if min_fd >= self.table.len() {
                if self.table.len() >= self.max_fd {
                    return Err(Errno::EMFILE);
                }
                self.set(min_fd, file, flags)?;
                Ok(min_fd)
            } else {
                self.push(file, flags)
            }
        }
    }

    pub fn dup2(&mut self, oldfd: usize, flags: FDFlags) -> SysResult<usize> {
        let file = self.get(oldfd)?;
        self.push(file, flags)
    }

    pub fn dup3(&mut self, oldfd: usize, newfd: usize, flags: FDFlags) -> SysResult<usize> {
        if oldfd == newfd {
            return Err(Errno::EINVAL);
        }

        if newfd >= config::MAX_FD {
            return Err(Errno::EBADF);
        }
        if newfd >= self.table.len() {
            self.table.resize(newfd + 1, None);
        }
        if oldfd >= self.table.len() {
            return Err(Errno::EBADF);
        }

        let file = self.table[oldfd].as_ref().ok_or(Errno::EBADF)?.file.clone();
        self.set(newfd, file, flags)?;

        Ok(newfd)
    }

    pub fn take(&mut self, fd: usize) -> SysResult<Arc<dyn FileOps>> {
        if fd < self.table.len() {
            if self.table[fd].is_none() {
                return Err(Errno::EBADF);
            }
            let file = self.table[fd].take().unwrap().file;
            if current::has_task() {
                release_posix_locks_from_file(&file, current::pid());
            }
            file.on_fd_remove();
            Ok(file)
        } else {
            Err(Errno::EBADF)
        }
    }

    pub fn fork(&self, owner: Tid) -> Self {
        let new_table = self
            .table
            .iter()
            .map(|item| item.as_ref().map(|fd_item| fd_item.clone()))
            .collect();

        self.table.iter().flatten().for_each(|item| {
            item.file.on_fd_install();
        });

        Self {
            table: new_table,
            max_fd: self.max_fd,
            owner,
        }
    }

    pub fn cloexec(&mut self) {
        self.table.iter_mut().for_each(|item| {
            if let Some(fd_item) = item {
                if fd_item.flags.cloexec {
                    release_posix_locks_from_file(&fd_item.file, current::pid());
                    fd_item.file.on_fd_remove();
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

    pub fn open_fds(&self) -> Vec<usize> {
        self.table
            .iter()
            .enumerate()
            .filter_map(|(fd, item)| item.as_ref().map(|_| fd))
            .collect()
    }
}

impl Drop for FDTable {
    fn drop(&mut self) {
        self.table.iter_mut().for_each(|item| {
            let Some(fd_item) = item.take() else {
                return;
            };
            release_posix_locks_from_file(&fd_item.file, self.owner);
            fd_item.file.on_fd_remove();
        });
    }
}
