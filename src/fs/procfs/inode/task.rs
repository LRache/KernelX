use alloc::string::String;
use alloc::sync::Arc;
use core::cmp::min;
use core::fmt::Write;

use crate::fs::file::{DirResult, File, FileFlags, FileOps};
use crate::fs::procfs::inode::{fill_kstat_common, read_iter_text};
use crate::fs::{Dentry, FileType, InodeOps, Mode};
use crate::kernel::errno::{Errno, SysResult};
use crate::kernel::mm::MapPerm;
use crate::kernel::scheduler::Tid;
use crate::kernel::task::manager;
use crate::kernel::uapi::{FileStat, Uid};

use super::RootInode;

pub struct TaskDirInode {
    tid: Tid,
}

impl TaskDirInode {
    pub const BASE_INO: u32 = 0x100000;

    pub fn from_ino(ino: u32) -> Option<Self> {
        debug_assert!(ino >= Self::BASE_INO);
        let tid = (ino - Self::BASE_INO) as Tid;
        manager::get(tid)?;
        Some(Self { tid })
    }

    pub fn ino_from_tid(tid: Tid) -> u32 {
        Self::BASE_INO + tid as u32
    }
}

impl InodeOps for TaskDirInode {
    fn get_ino(&self) -> u32 {
        Self::ino_from_tid(self.tid)
    }

    fn type_name(&self) -> &'static str {
        "procfs_task_dir"
    }

    fn readat(&self, _buf: &mut [u8], _offset: usize) -> SysResult<usize> {
        Err(Errno::EISDIR)
    }

    fn writeat(&self, _buf: &[u8], _offset: usize) -> SysResult<usize> {
        Err(Errno::EROFS)
    }

    fn lookup(&self, name: &str) -> SysResult<u32> {
        match name {
            "." => Ok(Self::ino_from_tid(self.tid)),
            ".." => Ok(RootInode::INO),
            "maps" => Ok(TaskMapsInode::ino_from_tid(self.tid)),
            "exe" => Ok(TaskExeInode::ino_from_tid(self.tid)),
            _ => Err(Errno::ENOENT)
        }
    }

    fn get_dent(&self, index: usize) -> SysResult<Option<(DirResult, usize)>> {
        let d = match index {
            0 => Some(DirResult { ino: Self::ino_from_tid(self.tid), name: ".".into(), file_type: FileType::Directory}),
            1 => Some(DirResult { ino: RootInode::INO, name: "..".into(), file_type: FileType::Directory}),
            2 => Some(DirResult { ino: TaskMapsInode::ino_from_tid(self.tid), name: "maps".into(), file_type: FileType::Regular}),
            3 => Some(DirResult { ino: TaskExeInode::ino_from_tid(self.tid), name: "exe".into(), file_type: FileType::Symlink}),
            _ => None,
        };

        Ok(d.map(|r| (r, index + 1)))
    }

    fn fstat(&self) -> SysResult<FileStat> {
        let mut kstat = FileStat::default();
        kstat.st_ino = self.get_ino() as u64;
        kstat.st_mode = self.mode()?.bits();
        kstat.st_nlink = 1;

        let pcb = manager::get(self.tid).ok_or(Errno::ESRCH)?;
        fill_kstat_common(&mut kstat, &pcb.first_task());
        
        Ok(kstat)
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
        let dentry = dentry.expect("procfs task dir requires associated dentry");
        Arc::new(File::new(self, dentry, flags))
    }
}

pub struct TaskMapsInode {
    tid: Tid
}

impl TaskMapsInode {
    pub const INO_BASE: u32 = 0x200000;

    pub fn from_ino(ino: u32) -> Option<Self> {
        debug_assert!(ino >= Self::INO_BASE);
        let tid = (ino - Self::INO_BASE) as Tid;
        manager::get(tid)?;
        Some(Self { tid })
    }

    fn ino_from_tid(tid: Tid) -> u32 {
        Self::INO_BASE + tid as u32
    }

    fn perm_string(perm: MapPerm) -> String {
        let mut perms = String::with_capacity(4);
        perms.push(if perm.contains(MapPerm::R) { 'r' } else { '-' });
        perms.push(if perm.contains(MapPerm::W) { 'w' } else { '-' });
        perms.push(if perm.contains(MapPerm::X) { 'x' } else { '-' });
        perms.push('p');
        perms
    }
}

impl InodeOps for TaskMapsInode {
    fn get_ino(&self) -> u32 {
        Self::ino_from_tid(self.tid)
    }

    fn type_name(&self) -> &'static str {
        "procfs_task_maps"
    }

    fn readat(&self, buf: &mut [u8], offset: usize) -> SysResult<usize> {
        let pcb = manager::get(self.tid).ok_or(Errno::ESRCH)?;
        let addrspace = pcb.first_task().get_addrspace().clone();
        let areas = addrspace.with_map_manager_mut(|manager| manager.snapshot());

        read_iter_text(buf, offset, areas.iter(), |area| {
            let perms = Self::perm_string(area.perm);
            let mut line = String::with_capacity(50);
            let _ = writeln!(
                line,
                "{:016x}-{:016x} {} {}",
                area.start,
                area.end,
                perms,
                area.name
            );
            Ok(line)
        })

        // let mut pos = 0usize;
        // let mut copied = 0usize;

        // for area in areas {
        //     let mut line = String::with_capacity(50);
        //     let perms = Self::perm_string(area.perm);
        //     let _ = writeln!(
        //         line,
        //         "{:016x}-{:016x} {} {}",
        //         area.start,
        //         area.end,
        //         perms,
        //         area.name
        //     );

        //     let line_len = line.len();
        //     if pos + line_len <= offset {
        //         pos += line_len;
        //         continue;
        //     }

        //     if copied >= buf.len() {
        //         break;
        //     }

        //     let line_bytes = line.as_bytes();
        //     let start_in_line = offset.saturating_sub(pos);
        //     let left_in_line = line_len.saturating_sub(start_in_line);
        //     let to_copy = min(left_in_line, buf.len() - copied);
        //     if to_copy == 0 {
        //         pos += line_len;
        //         continue;
        //     }

        //     buf[copied..copied + to_copy]
        //         .copy_from_slice(&line_bytes[start_in_line..start_in_line + to_copy]);
        //     copied += to_copy;
        //     pos += line_len;

        //     if copied == buf.len() {
        //         break;
        //     }
        // }

        // Ok(copied)
    }

    fn writeat(&self, _buf: &[u8], _offset: usize) -> SysResult<usize> {
        Err(Errno::EROFS)
    }

    fn fstat(&self) -> SysResult<FileStat> {
        let mut kstat = FileStat::default();
        kstat.st_ino = self.get_ino() as u64;
        kstat.st_mode = self.mode()?.bits();
        kstat.st_nlink = 1;

        let pcb = manager::get(self.tid).ok_or(Errno::ESRCH)?;
        fill_kstat_common(&mut kstat, &pcb.first_task());
        
        Ok(kstat)
    }

    fn mode(&self) -> SysResult<Mode> {
        Ok(Mode::S_IFREG | Mode::S_IRUSR | Mode::S_IRGRP | Mode::S_IROTH)
    }

    fn size(&self) -> SysResult<u64> {
        Ok(0)
    }

    fn wrap_file(self: Arc<Self>, dentry: Option<Arc<Dentry>>, flags: FileFlags) -> Arc<dyn FileOps> {
        let dentry = dentry.expect("procfs maps requires associated dentry");
        Arc::new(File::new(self, dentry, flags))
    }
}

pub struct TaskExeInode {
    tid: Tid
}

impl TaskExeInode {
    pub const INO_BASE: u32 = 0x300000;

    pub fn from_ino(ino: u32) -> Option<Self> {
        debug_assert!(ino >= Self::INO_BASE);
        let tid = (ino - Self::INO_BASE) as Tid;
        manager::get(tid)?;
        Some(Self { tid })
    }

    fn ino_from_tid(tid: Tid) -> u32 {
        Self::INO_BASE + tid as u32
    }
}

impl InodeOps for TaskExeInode {
    fn get_ino(&self) -> u32 {
        Self::ino_from_tid(self.tid)
    }

    fn readat(&self, _buf: &mut [u8], _offset: usize) -> SysResult<usize> {
        unreachable!()
    }

    fn writeat(&self, _buf: &[u8], _offset: usize) -> SysResult<usize> {
        unreachable!()
    }

    fn create(&self, _name: &str, _mode: Mode) -> SysResult<Arc<dyn InodeOps>> {
        Err(Errno::ENOTDIR)
    }

    fn readlink(&self, buf: &mut [u8]) -> SysResult<Option<usize>> {
        let pcb = manager::get(self.tid).ok_or(Errno::ESRCH)?;
        let exe_path = pcb.exec_path();
        let exe_path_bytes = exe_path.as_bytes();
        let to_copy = min(buf.len(), exe_path_bytes.len());
        buf[..to_copy].copy_from_slice(&exe_path_bytes[..to_copy]);
        Ok(Some(to_copy))
    }

    fn size(&self) -> SysResult<u64> {
        Ok(0)
    }

    fn mode(&self) -> SysResult<Mode> {
        Ok(Mode::S_IFLNK | Mode::S_IRUSR | Mode::S_IRGRP | Mode::S_IROTH)
    }

    fn fstat(&self) -> SysResult<FileStat> {
        let mut kstat = FileStat::default();
        kstat.st_ino = self.get_ino() as u64;
        kstat.st_mode = self.mode()?.bits();
        kstat.st_nlink = 1;
        
        let pcb = manager::get(self.tid).ok_or(Errno::ESRCH)?;
        fill_kstat_common(&mut kstat, &pcb.first_task());

        Ok(kstat)
    }

    fn owner(&self) -> SysResult<(Uid, Uid)> {
        Ok((0, 0))
    }

    fn wrap_file(self: Arc<Self>, dentry: Option<Arc<Dentry>>, flags: FileFlags) -> Arc<dyn FileOps> {
        Arc::new(File::new(self, dentry.unwrap(), flags))
    }

    fn type_name(&self) -> &'static str {
        "procfs_task_exe"
    }
}
