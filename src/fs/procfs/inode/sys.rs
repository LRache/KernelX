use alloc::string::String;
use alloc::sync::Arc;
use core::cmp::min;
use core::fmt::Write;

use crate::fs::file::{DirResult, FileFlags, FileOps, RandomAccessFile};
use crate::fs::{Dentry, FileType, InodeOps, Mode};
use crate::kernel::config;
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

    fn readat(&self, _buf: &mut [u8], _offset: usize, _direct: bool) -> SysResult<usize> {
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
            "fs" => Ok(SysFsDirInode::INO),
            "vm" => Ok(SysVmDirInode::INO),
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
            3 => Some(DirResult {
                ino: SysFsDirInode::INO,
                name: "fs".into(),
                file_type: FileType::Directory,
            }),
            4 => Some(DirResult {
                ino: SysVmDirInode::INO,
                name: "vm".into(),
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
        Arc::new(RandomAccessFile::new(self, dentry, flags))
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

    fn readat(&self, _buf: &mut [u8], _offset: usize, _direct: bool) -> SysResult<usize> {
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
            "random" => Ok(SysKernelRandomDirInode::INO),
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
            4 => Some(DirResult {
                ino: SysKernelRandomDirInode::INO,
                name: "random".into(),
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
        let dentry = dentry.expect("procfs sys kernel dir requires associated dentry");
        Arc::new(RandomAccessFile::new(self, dentry, flags))
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

    fn readat(&self, buf: &mut [u8], offset: usize, _direct: bool) -> SysResult<usize> {
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
        Arc::new(RandomAccessFile::new(self, dentry.unwrap(), flags))
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

    fn readat(&self, buf: &mut [u8], offset: usize, _direct: bool) -> SysResult<usize> {
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
        Arc::new(RandomAccessFile::new(self, dentry.unwrap(), flags))
    }
}

// /proc/sys/kernel/random/
pub struct SysKernelRandomDirInode;

impl SysKernelRandomDirInode {
    pub const INO: u32 = 15;
}

impl InodeOps for SysKernelRandomDirInode {
    fn get_ino(&self) -> u32 {
        Self::INO
    }

    fn type_name(&self) -> &'static str {
        "procfs_sys_kernel_random"
    }

    fn readat(&self, _buf: &mut [u8], _offset: usize, _direct: bool) -> SysResult<usize> {
        Err(Errno::EISDIR)
    }

    fn writeat(&self, _buf: &[u8], _offset: usize) -> SysResult<usize> {
        Err(Errno::EROFS)
    }

    fn lookup(&self, name: &str) -> SysResult<u32> {
        match name {
            "." => Ok(Self::INO),
            ".." => Ok(SysKernelDirInode::INO),
            "entropy_avail" => Ok(EntropyAvailInode::INO),
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
                ino: SysKernelDirInode::INO,
                name: "..".into(),
                file_type: FileType::Directory,
            }),
            2 => Some(DirResult {
                ino: EntropyAvailInode::INO,
                name: "entropy_avail".into(),
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
        let dentry = dentry.expect("procfs sys kernel random dir requires associated dentry");
        Arc::new(RandomAccessFile::new(self, dentry, flags))
    }
}

// /proc/sys/kernel/random/entropy_avail
pub struct EntropyAvailInode;

impl EntropyAvailInode {
    pub const INO: u32 = 16;
}

impl InodeOps for EntropyAvailInode {
    fn get_ino(&self) -> u32 {
        Self::INO
    }

    fn type_name(&self) -> &'static str {
        "procfs_entropy_avail"
    }

    fn readat(&self, buf: &mut [u8], offset: usize, _direct: bool) -> SysResult<usize> {
        let content = b"256\n";
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
        Arc::new(RandomAccessFile::new(self, dentry.unwrap(), flags))
    }
}

// /proc/sys/fs/
pub struct SysFsDirInode;

impl SysFsDirInode {
    pub const INO: u32 = 9;
}

impl InodeOps for SysFsDirInode {
    fn get_ino(&self) -> u32 {
        Self::INO
    }

    fn type_name(&self) -> &'static str {
        "procfs_sys_fs"
    }

    fn readat(&self, _buf: &mut [u8], _offset: usize, _direct: bool) -> SysResult<usize> {
        Err(Errno::EISDIR)
    }

    fn writeat(&self, _buf: &[u8], _offset: usize) -> SysResult<usize> {
        Err(Errno::EROFS)
    }

    fn lookup(&self, name: &str) -> SysResult<u32> {
        match name {
            "." => Ok(Self::INO),
            ".." => Ok(SysDirInode::INO),
            "pipe-max-size" => Ok(PipeMaxSizeInode::INO),
            "pipe-user-pages-soft" => Ok(PipeUserPagesSoftInode::INO),
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
                ino: PipeMaxSizeInode::INO,
                name: "pipe-max-size".into(),
                file_type: FileType::Regular,
            }),
            3 => Some(DirResult {
                ino: PipeUserPagesSoftInode::INO,
                name: "pipe-user-pages-soft".into(),
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
        let dentry = dentry.expect("procfs sys fs dir requires associated dentry");
        Arc::new(RandomAccessFile::new(self, dentry, flags))
    }
}

// /proc/sys/fs/pipe-max-size
pub struct PipeMaxSizeInode;

impl PipeMaxSizeInode {
    pub const INO: u32 = 10;
}

impl InodeOps for PipeMaxSizeInode {
    fn get_ino(&self) -> u32 {
        Self::INO
    }

    fn type_name(&self) -> &'static str {
        "procfs_pipe_max_size"
    }

    fn readat(&self, buf: &mut [u8], offset: usize, _direct: bool) -> SysResult<usize> {
        let mut content = String::with_capacity(16);
        let _ = writeln!(content, "{}", config::PIPE_CAPACITY);
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
        Arc::new(RandomAccessFile::new(self, dentry.unwrap(), flags))
    }
}

// /proc/sys/fs/pipe-user-pages-soft
pub struct PipeUserPagesSoftInode;

impl PipeUserPagesSoftInode {
    pub const INO: u32 = 11;
}

impl InodeOps for PipeUserPagesSoftInode {
    fn get_ino(&self) -> u32 {
        Self::INO
    }

    fn type_name(&self) -> &'static str {
        "procfs_pipe_user_pages_soft"
    }

    fn readat(&self, buf: &mut [u8], offset: usize, _direct: bool) -> SysResult<usize> {
        let content = b"16384\n";
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
        Arc::new(RandomAccessFile::new(self, dentry.unwrap(), flags))
    }
}

// /proc/sys/vm/
pub struct SysVmDirInode;

impl SysVmDirInode {
    pub const INO: u32 = 12;
}

impl InodeOps for SysVmDirInode {
    fn get_ino(&self) -> u32 {
        Self::INO
    }

    fn type_name(&self) -> &'static str {
        "procfs_sys_vm"
    }

    fn readat(&self, _buf: &mut [u8], _offset: usize, _direct: bool) -> SysResult<usize> {
        Err(Errno::EISDIR)
    }

    fn writeat(&self, _buf: &[u8], _offset: usize) -> SysResult<usize> {
        Err(Errno::EROFS)
    }

    fn lookup(&self, name: &str) -> SysResult<u32> {
        match name {
            "." => Ok(Self::INO),
            ".." => Ok(SysDirInode::INO),
            "vfs_cache_pressure" => Ok(VfsCachePressureInode::INO),
            "drop_caches" => Ok(DropCachesInode::INO),
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
                ino: VfsCachePressureInode::INO,
                name: "vfs_cache_pressure".into(),
                file_type: FileType::Regular,
            }),
            3 => Some(DirResult {
                ino: DropCachesInode::INO,
                name: "drop_caches".into(),
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
        let dentry = dentry.expect("procfs sys vm dir requires associated dentry");
        Arc::new(RandomAccessFile::new(self, dentry, flags))
    }
}

// /proc/sys/vm/vfs_cache_pressure
pub struct VfsCachePressureInode;

impl VfsCachePressureInode {
    pub const INO: u32 = 13;
}

impl InodeOps for VfsCachePressureInode {
    fn get_ino(&self) -> u32 {
        Self::INO
    }

    fn type_name(&self) -> &'static str {
        "procfs_vfs_cache_pressure"
    }

    fn readat(&self, buf: &mut [u8], offset: usize, _direct: bool) -> SysResult<usize> {
        let content = b"100\n";
        if offset >= content.len() {
            return Ok(0);
        }
        let len = min(buf.len(), content.len() - offset);
        buf[..len].copy_from_slice(&content[offset..offset + len]);
        Ok(len)
    }

    fn writeat(&self, buf: &[u8], _offset: usize) -> SysResult<usize> {
        Ok(buf.len())
    }

    fn truncate(&self, _new_size: u64) -> SysResult<()> {
        Ok(())
    }

    fn mode(&self) -> SysResult<Mode> {
        Ok(Mode::S_IFREG | Mode::S_IRUSR | Mode::S_IWUSR | Mode::S_IRGRP | Mode::S_IROTH)
    }

    fn size(&self) -> SysResult<u64> {
        Ok(0)
    }

    fn wrap_file(self: Arc<Self>, dentry: Option<Arc<Dentry>>, flags: FileFlags) -> Arc<dyn FileOps> {
        Arc::new(RandomAccessFile::new(self, dentry.unwrap(), flags))
    }
}

// /proc/sys/vm/drop_caches
pub struct DropCachesInode;

impl DropCachesInode {
    pub const INO: u32 = 14;
}

impl InodeOps for DropCachesInode {
    fn get_ino(&self) -> u32 {
        Self::INO
    }

    fn type_name(&self) -> &'static str {
        "procfs_drop_caches"
    }

    fn readat(&self, buf: &mut [u8], offset: usize, _direct: bool) -> SysResult<usize> {
        let content = b"0\n";
        if offset >= content.len() {
            return Ok(0);
        }
        let len = min(buf.len(), content.len() - offset);
        buf[..len].copy_from_slice(&content[offset..offset + len]);
        Ok(len)
    }

    fn writeat(&self, buf: &[u8], _offset: usize) -> SysResult<usize> {
        Ok(buf.len())
    }

    fn truncate(&self, _new_size: u64) -> SysResult<()> {
        Ok(())
    }

    fn mode(&self) -> SysResult<Mode> {
        Ok(Mode::S_IFREG | Mode::S_IRUSR | Mode::S_IWUSR | Mode::S_IRGRP | Mode::S_IROTH)
    }

    fn size(&self) -> SysResult<u64> {
        Ok(0)
    }

    fn wrap_file(self: Arc<Self>, dentry: Option<Arc<Dentry>>, flags: FileFlags) -> Arc<dyn FileOps> {
        Arc::new(RandomAccessFile::new(self, dentry.unwrap(), flags))
    }
}
