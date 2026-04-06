use alloc::string::String;
use alloc::sync::Arc;
use core::cmp::min;
use core::fmt::Write;

use crate::fs::file::{DirResult, File, FileFlags, FileOps};
use crate::fs::{Dentry, FileType, InodeOps, Mode};
use crate::kernel::errno::{Errno, SysResult};
use crate::kernel::scheduler::tid::PID_MAX;

use super::RootInode;

// /proc/sys/
pub struct SysDirInode;

impl SysDirInode {
    pub const INO: u32 = 5;
}

impl InodeOps for SysDirInode {
    fn get_ino(&self) -> u32 {
        Self::INO
    }

    fn type_name(&self) -> &'static str {
        "procfs_sys"
    }

    fn readat(&self, _buf: &mut [u8], _offset: usize) -> SysResult<usize> {
        Err(Errno::EISDIR)
    }

    fn writeat(&self, _buf: &[u8], _offset: usize) -> SysResult<usize> {
        Err(Errno::EROFS)
    }

    fn lookup(&self, name: &str) -> SysResult<u32> {
        match name {
            "." => Ok(Self::INO),
            ".." => Ok(RootInode::INO),
            "kernel" => Ok(SysKernelDirInode::INO),
            _ => Err(Errno::ENOENT),
        }
    }

    fn get_dent(&self, index: usize) -> SysResult<Option<(DirResult, usize)>> {
        let d = match index {
            0 => Some(DirResult {
                ino: Self::INO,
                name: ".".into(),
                file_type: FileType::Directory,
            }),
            1 => Some(DirResult {
                ino: RootInode::INO,
                name: "..".into(),
                file_type: FileType::Directory,
            }),
            2 => Some(DirResult {
                ino: SysKernelDirInode::INO,
                name: "kernel".into(),
                file_type: FileType::Directory,
            }),
            _ => None,
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
        let dentry = dentry.expect("procfs sys dir requires associated dentry");
        Arc::new(File::new(self, dentry, flags))
    }
}

// /proc/sys/kernel/
pub struct SysKernelDirInode;

impl SysKernelDirInode {
    pub const INO: u32 = 6;
}

impl InodeOps for SysKernelDirInode {
    fn get_ino(&self) -> u32 {
        Self::INO
    }

    fn type_name(&self) -> &'static str {
        "procfs_sys_kernel"
    }

    fn readat(&self, _buf: &mut [u8], _offset: usize) -> SysResult<usize> {
        Err(Errno::EISDIR)
    }

    fn writeat(&self, _buf: &[u8], _offset: usize) -> SysResult<usize> {
        Err(Errno::EROFS)
    }

    fn lookup(&self, name: &str) -> SysResult<u32> {
        match name {
            "." => Ok(Self::INO),
            ".." => Ok(SysDirInode::INO),
            "pid_max" => Ok(PidMaxInode::INO),
            "tainted" => Ok(TaintedInode::INO),
            _ => Err(Errno::ENOENT),
        }
    }

    fn get_dent(&self, index: usize) -> SysResult<Option<(DirResult, usize)>> {
        let d = match index {
            0 => Some(DirResult {
                ino: Self::INO,
                name: ".".into(),
                file_type: FileType::Directory,
            }),
            1 => Some(DirResult {
                ino: SysDirInode::INO,
                name: "..".into(),
                file_type: FileType::Directory,
            }),
            2 => Some(DirResult {
                ino: PidMaxInode::INO,
                name: "pid_max".into(),
                file_type: FileType::Regular,
            }),
            3 => Some(DirResult {
                ino: TaintedInode::INO,
                name: "tainted".into(),
                file_type: FileType::Regular,
            }),
            _ => None,
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
        let dentry = dentry.expect("procfs sys kernel dir requires associated dentry");
        Arc::new(File::new(self, dentry, flags))
    }
}

// /proc/sys/kernel/pid_max
pub struct PidMaxInode;

impl PidMaxInode {
    pub const INO: u32 = 7;
}

impl InodeOps for PidMaxInode {
    fn get_ino(&self) -> u32 {
        Self::INO
    }

    fn type_name(&self) -> &'static str {
        "procfs_pid_max"
    }

    fn readat(&self, buf: &mut [u8], offset: usize) -> SysResult<usize> {
        let mut content = String::with_capacity(16);
        let _ = writeln!(content, "{}", PID_MAX);
        let bytes = content.as_bytes();
        if offset >= bytes.len() {
            return Ok(0);
        }
        let len = min(buf.len(), bytes.len() - offset);
        buf[..len].copy_from_slice(&bytes[offset..offset + len]);
        Ok(len)
    }

    fn writeat(&self, _buf: &[u8], _offset: usize) -> SysResult<usize> {
        Err(Errno::EROFS)
    }

    fn mode(&self) -> SysResult<Mode> {
        Ok(Mode::S_IFREG | Mode::S_IRUSR | Mode::S_IRGRP | Mode::S_IROTH)
    }

    fn size(&self) -> SysResult<u64> {
        Ok(0)
    }

    fn wrap_file(self: Arc<Self>, dentry: Option<Arc<Dentry>>, flags: FileFlags) -> Arc<dyn FileOps> {
        Arc::new(File::new(self, dentry.unwrap(), flags))
    }
}

// /proc/sys/kernel/tainted
pub struct TaintedInode;

impl TaintedInode {
    pub const INO: u32 = 8;
}

impl InodeOps for TaintedInode {
    fn get_ino(&self) -> u32 {
        Self::INO
    }

    fn type_name(&self) -> &'static str {
        "procfs_tainted"
    }

    fn readat(&self, buf: &mut [u8], offset: usize) -> SysResult<usize> {
        let content = b"0\n";
        if offset >= content.len() {
            return Ok(0);
        }
        let len = min(buf.len(), content.len() - offset);
        buf[..len].copy_from_slice(&content[offset..offset + len]);
        Ok(len)
    }

    fn writeat(&self, _buf: &[u8], _offset: usize) -> SysResult<usize> {
        Err(Errno::EROFS)
    }

    fn mode(&self) -> SysResult<Mode> {
        Ok(Mode::S_IFREG | Mode::S_IRUSR | Mode::S_IRGRP | Mode::S_IROTH)
    }

    fn size(&self) -> SysResult<u64> {
        Ok(0)
    }

    fn wrap_file(self: Arc<Self>, dentry: Option<Arc<Dentry>>, flags: FileFlags) -> Arc<dyn FileOps> {
        Arc::new(File::new(self, dentry.unwrap(), flags))
    }
}
