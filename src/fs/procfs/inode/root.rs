use alloc::string::{String, ToString};
use alloc::sync::Arc;
use core::fmt::Write;

use crate::fs::file::{DirResult, File, FileFlags, FileOps};
use crate::fs::procfs::inode::read_iter_text;
use crate::fs::vfs::vfs;
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
            "mounts" => Ok(MountsInode::INO),
            _ => {
                let tid = name.parse::<Tid>().map_err(|_| Errno::ENOENT)?;
                Self::task_dir_ino_from_tid(tid)
            }
        }
    }

    fn get_dent(&self, index: usize) -> SysResult<Option<(DirResult, usize)>> {
        const SPECIAL_ENTRIES: usize = 4; // ., .., self, mounts
        let d = match index {
            0 => Some(DirResult { ino: Self::INO, name: ".".into(), file_type: FileType::Directory}),
            1 => Some(DirResult { ino: Self::INO, name: "..".into(), file_type: FileType::Directory}),
            2 => Some(DirResult { ino: TaskDirSelfInode::INO, name: "self".into(), file_type: FileType::Symlink}),
            3 => Some(DirResult { ino: MountsInode::INO, name: "mounts".into(), file_type: FileType::Regular}),
            i => {
                manager::pcbs().lock().iter().nth(i - SPECIAL_ENTRIES).map(|(&pid, _)| {
                    DirResult {
                        ino: TaskDirInode::ino_from_tid(pid),
                        name: pid.to_string(),
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

pub struct MountsInode;

impl MountsInode {
    pub const INO: u32 = 3;
}

impl InodeOps for MountsInode {
    fn get_ino(&self) -> u32 {
        Self::INO
    }

    fn type_name(&self) -> &'static str {
        "procfs_mounts"
    }

    fn readat(&self, buf: &mut [u8], offset: usize) -> SysResult<usize> {
        let mounts = vfs().mountpoint_list();
        read_iter_text(buf, offset, mounts.iter(), |dentry| {
            let mut line = String::with_capacity(50);
            let path = dentry.get_path();
            let mount_to = dentry.clone().get_mount_to();
            let mount_type = mount_to.get_inode().type_name();
            let _ = writeln!(
                line,
                "{} {} {} {} 0 0",
                "device", // Dummy device name
                path,
                mount_type,
                "rw" // Dummy mount options
            );

            Ok(line)
        })
    }

    fn writeat(&self, _buf: &[u8], _offset: usize) -> SysResult<usize> {
        Err(Errno::EROFS)
    }

    fn mode(&self) -> SysResult<Mode> {
        Ok(Mode::S_IFREG
            | Mode::S_IRUSR
            | Mode::S_IRGRP
            | Mode::S_IROTH)
    }

    fn wrap_file(self: Arc<Self>, dentry: Option<Arc<Dentry>>, flags: FileFlags) -> Arc<dyn FileOps> {
        Arc::new(File::new(self, dentry.unwrap(), flags))
    }

    fn size(&self) -> SysResult<u64> {
        Ok(0)
    }
}
