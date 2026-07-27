use alloc::string::{String, ToString};
use alloc::sync::Arc;
use core::fmt::Write;

use crate::arch;
use crate::fs::file::{DirResult, FileFlags, FileOps, RandomAccessFile};
use crate::fs::procfs::inode::read_iter_text;
use crate::fs::vfs::vfs;
use crate::fs::{Dentry, FileType, Inode, InodeOps, Mode};
use crate::kernel::errno::{Errno, SysResult};
use crate::kernel::mm::page;
use crate::kernel::scheduler::Tid;
use crate::kernel::scheduler::tid::TID_START;
use crate::kernel::task::manager;

use super::{SysDirInode, TaskDirInode, TaskDirSelfInode};

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

    fn readat(&self, _buf: &mut [u8], _offset: usize, _direct: bool) -> SysResult<usize> {
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
            "meminfo" => Ok(MemInfoInode::INO),
            "sys" => Ok(SysDirInode::INO),
            "uptime" => Ok(UptimeInode::INO),
            _ => {
                let tid = name.parse::<Tid>().map_err(|_| Errno::ENOENT)?;
                Self::task_dir_ino_from_tid(tid)
            }
        }
    }

    fn get_dent(&self, index: usize) -> SysResult<Option<(DirResult, usize)>> {
        const SPECIAL_ENTRIES: usize = 7; // ., .., self, mounts, meminfo, sys, uptime
        let d = match index {
            0 => Some(DirResult {
                ino: Self::INO,
                name: ".".into(),
                file_type: FileType::Directory,
            }),
            1 => Some(DirResult {
                ino: Self::INO,
                name: "..".into(),
                file_type: FileType::Directory,
            }),
            2 => Some(DirResult {
                ino: TaskDirSelfInode::INO,
                name: "self".into(),
                file_type: FileType::Symlink,
            }),
            3 => Some(DirResult {
                ino: MountsInode::INO,
                name: "mounts".into(),
                file_type: FileType::Regular,
            }),
            4 => Some(DirResult {
                ino: MemInfoInode::INO,
                name: "meminfo".into(),
                file_type: FileType::Regular,
            }),
            5 => Some(DirResult {
                ino: SysDirInode::INO,
                name: "sys".into(),
                file_type: FileType::Directory,
            }),
            6 => Some(DirResult {
                ino: UptimeInode::INO,
                name: "uptime".into(),
                file_type: FileType::Regular,
            }),
            i => manager::tcbs()
                .lock()
                .iter()
                .nth(i - SPECIAL_ENTRIES)
                .map(|(&pid, _)| DirResult {
                    ino: TaskDirInode::ino_from_tid(pid),
                    name: pid.to_string(),
                    file_type: FileType::Directory,
                }),
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

    fn wrap_file(&self, inode: Arc<Inode>, dentry: Option<Arc<Dentry>>, flags: FileFlags) -> Arc<dyn FileOps> {
        let dentry = dentry.expect("procfs root requires associated dentry");
        Arc::new(RandomAccessFile::new(inode, dentry, flags))
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

    fn readat(&self, buf: &mut [u8], offset: usize, _direct: bool) -> SysResult<usize> {
        let mounts = vfs().mountpoint_list();
        read_iter_text(buf, offset, mounts.iter(), |dentry| {
            let mut line = String::with_capacity(50);
            let path = dentry.get_path();
            let mount_to = dentry.clone().get_mount_to();
            let mount_type = mount_to
                .get_inode()
                .map(|inode| inode.type_name())
                .unwrap_or("unknown");
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
        Ok(Mode::S_IFREG | Mode::S_IRUSR | Mode::S_IRGRP | Mode::S_IROTH)
    }

    fn wrap_file(&self, inode: Arc<Inode>, dentry: Option<Arc<Dentry>>, flags: FileFlags) -> Arc<dyn FileOps> {
        Arc::new(RandomAccessFile::new(inode, dentry.unwrap(), flags))
    }

    fn size(&self) -> SysResult<u64> {
        Ok(0)
    }
}

pub struct MemInfoInode;

impl MemInfoInode {
    pub const INO: u32 = 4;
}

impl InodeOps for MemInfoInode {
    fn get_ino(&self) -> u32 {
        Self::INO
    }

    fn type_name(&self) -> &'static str {
        "procfs_meminfo"
    }

    fn readat(&self, buf: &mut [u8], offset: usize, _direct: bool) -> SysResult<usize> {
        let total_kb = page::total_pages() * arch::PGSIZE / 1024;
        let available_kb = page::free_pages() * arch::PGSIZE / 1024;
        let mut content = String::with_capacity(128);
        let _ = writeln!(content, "MemTotal:       {} kB", total_kb);
        let _ = writeln!(content, "MemAvailable:   {} kB", available_kb);

        let bytes = content.as_bytes();
        if offset >= bytes.len() {
            return Ok(0);
        }
        let len = core::cmp::min(buf.len(), bytes.len() - offset);
        buf[..len].copy_from_slice(&bytes[offset..offset + len]);
        Ok(len)
    }

    fn writeat(&self, _buf: &[u8], _offset: usize) -> SysResult<usize> {
        Err(Errno::EROFS)
    }

    fn mode(&self) -> SysResult<Mode> {
        Ok(Mode::S_IFREG | Mode::S_IRUSR | Mode::S_IRGRP | Mode::S_IROTH)
    }

    fn wrap_file(&self, inode: Arc<Inode>, dentry: Option<Arc<Dentry>>, flags: FileFlags) -> Arc<dyn FileOps> {
        Arc::new(RandomAccessFile::new(inode, dentry.unwrap(), flags))
    }

    fn size(&self) -> SysResult<u64> {
        Ok(0)
    }
}

pub struct UptimeInode;

impl UptimeInode {
    pub const INO: u32 = 17;
}

impl InodeOps for UptimeInode {
    fn get_ino(&self) -> u32 {
        Self::INO
    }

    fn type_name(&self) -> &'static str {
        "procfs_uptime"
    }

    fn readat(&self, buf: &mut [u8], offset: usize, _direct: bool) -> SysResult<usize> {
        let uptime = arch::uptime();
        let mut content = String::with_capacity(32);
        // TODO: Report the accumulated CPU idle time in the second field.
        let _ = writeln!(content, "{}.{:02} 0.00", uptime.as_secs(), uptime.subsec_millis() / 10);

        let bytes = content.as_bytes();
        if offset >= bytes.len() {
            return Ok(0);
        }
        let len = core::cmp::min(buf.len(), bytes.len() - offset);
        buf[..len].copy_from_slice(&bytes[offset..offset + len]);
        Ok(len)
    }

    fn writeat(&self, _buf: &[u8], _offset: usize) -> SysResult<usize> {
        Err(Errno::EROFS)
    }

    fn mode(&self) -> SysResult<Mode> {
        Ok(Mode::S_IFREG | Mode::S_IRUSR | Mode::S_IRGRP | Mode::S_IROTH)
    }

    fn wrap_file(&self, inode: Arc<Inode>, dentry: Option<Arc<Dentry>>, flags: FileFlags) -> Arc<dyn FileOps> {
        Arc::new(RandomAccessFile::new(inode, dentry.unwrap(), flags))
    }

    fn size(&self) -> SysResult<u64> {
        Ok(0)
    }
}
