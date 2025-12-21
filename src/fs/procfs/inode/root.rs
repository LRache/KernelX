use alloc::string::ToString;
use alloc::sync::Arc;

use crate::fs::file::{DirResult, File, FileFlags, FileOps};
use crate::fs::{Dentry, FileType, InodeOps, Mode};
use crate::kernel::errno::{Errno, SysResult};
use crate::kernel::scheduler::{tid::TID_START, Tid};
use crate::kernel::task::manager;

use super::{TaskDirInode, TaskDirSelfInode};

pub struct RootInode;

impl RootInode {
    pub const INO: u32 = 1;

    fn task_dir_ino_from_tid(tid: Tid) -> SysResult<u32> {
        if tid < TID_START {
            return Err(Errno::ENOENT);
        }

        if manager::get(tid).is_some() {
            Ok(TaskDirInode::ino_from_tid(tid))
        } else {
            Err(Errno::ENOENT)
        }
    }
}

impl InodeOps for RootInode {
    fn get_ino(&self) -> u32 {
        Self::INO
    }

    fn type_name(&self) -> &'static str {
        "procfs_root"
    }

    fn readat(&self, _buf: &mut [u8], _offset: usize) -> SysResult<usize> {
        Err(Errno::EISDIR)
    }

    fn writeat(&self, _buf: &[u8], _offset: usize) -> SysResult<usize> {
        Err(Errno::EISDIR)
    }

    fn lookup(&self, name: &str) -> SysResult<u32> {
        match name {
            "." => Ok(Self::INO),
            ".." => Ok(Self::INO),
            "self" => Ok(TaskDirSelfInode::INO),
            _ => {
                let tid = name.parse::<Tid>().map_err(|_| Errno::ENOENT)?;
                Self::task_dir_ino_from_tid(tid)
            }
        }
    }

    fn get_dent(&self, index: usize) -> SysResult<Option<(DirResult, usize)>> {
        const SPECIAL_ENTRIES: usize = 3; // ., .., self
        let d = match index {
            0 => Some(DirResult { ino: Self::INO, name: ".".into(), file_type: FileType::Directory}),
            1 => Some(DirResult { ino: Self::INO, name: "..".into(), file_type: FileType::Directory}),
            2 => Some(DirResult { ino: TaskDirSelfInode::INO, name: "self".into(), file_type: FileType::Symlink}),
            i => {
                manager::tcbs().lock().iter().nth(i - SPECIAL_ENTRIES).map(|(&tid, _)| {
                    DirResult {
                        ino: TaskDirInode::ino_from_tid(tid),
                        name: tid.to_string(),
                        file_type: FileType::Directory,
                    }
                })
            }
        };

        Ok(d.map(|r| (r, index + 1)))
    }

    fn mode(&self) -> SysResult<Mode> {
        Ok(Mode::S_IFDIR
            | Mode::S_IRUSR
            | Mode::S_IXUSR
            | Mode::S_IRGRP
            | Mode::S_IXGRP
            | Mode::S_IROTH
            | Mode::S_IXOTH)
    }

    fn size(&self) -> SysResult<u64> {
        Ok(0)
    }

    fn wrap_file(self: Arc<Self>, dentry: Option<Arc<Dentry>>, flags: FileFlags) -> Arc<dyn FileOps> {
        let dentry = dentry.expect("procfs root requires associated dentry");
        Arc::new(File::new(self, dentry, flags))
    }
}