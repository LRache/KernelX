use alloc::string::{String, ToString};
use alloc::sync::Arc;
use core::cmp::min;
use core::fmt::Write;

use crate::fs::file::{DirResult, File, FileFlags, FileOps};
use crate::fs::procfs::inode::{fill_kstat_common, read_iter_text};
use crate::fs::{Dentry, FileType, InodeOps, Mode, Owner};
use crate::kernel::errno::{Errno, SysResult};
use crate::kernel::mm::MapPerm;
use crate::kernel::scheduler::{TaskState, Tid};
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

    fn readat(&self, _buf: &mut [u8], _offset: usize, _direct: bool) -> SysResult<usize> {
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
            "stat" => Ok(TaskStatInode::ino_from_tid(self.tid)),
            "status" => Ok(TaskStatusInode::ino_from_tid(self.tid)),
            "fd" => Ok(TaskFdDirInode::ino_from_tid(self.tid)),
            _ => Err(Errno::ENOENT),
        }
    }

    fn get_dent(&self, index: usize) -> SysResult<Option<(DirResult, usize)>> {
        let d = match index {
            0 => Some(DirResult {
                ino: Self::ino_from_tid(self.tid),
                name: ".".into(),
                file_type: FileType::Directory,
            }),
            1 => Some(DirResult {
                ino: RootInode::INO,
                name: "..".into(),
                file_type: FileType::Directory,
            }),
            2 => Some(DirResult {
                ino: TaskMapsInode::ino_from_tid(self.tid),
                name: "maps".into(),
                file_type: FileType::Regular,
            }),
            3 => Some(DirResult {
                ino: TaskExeInode::ino_from_tid(self.tid),
                name: "exe".into(),
                file_type: FileType::Symlink,
            }),
            4 => Some(DirResult {
                ino: TaskStatInode::ino_from_tid(self.tid),
                name: "stat".into(),
                file_type: FileType::Regular,
            }),
            5 => Some(DirResult {
                ino: TaskStatusInode::ino_from_tid(self.tid),
                name: "status".into(),
                file_type: FileType::Regular,
            }),
            6 => Some(DirResult {
                ino: TaskFdDirInode::ino_from_tid(self.tid),
                name: "fd".into(),
                file_type: FileType::Directory,
            }),
            _ => None,
        };

        Ok(d.map(|r| (r, index + 1)))
    }

    fn fstat(&self) -> SysResult<FileStat> {
        let mut kstat = FileStat::default();
        kstat.st_ino = self.get_ino() as u64;
        kstat.st_mode = self.mode()?.bits();
        kstat.st_nlink = 1;

        let tcb = manager::get(self.tid).ok_or(Errno::ESRCH)?;
        fill_kstat_common(&mut kstat, &tcb);

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
    tid: Tid,
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

    fn readat(&self, buf: &mut [u8], offset: usize, _direct: bool) -> SysResult<usize> {
        let tcb = manager::get(self.tid).ok_or(Errno::ESRCH)?;
        let addrspace = tcb.get_addrspace().clone();
        let areas = addrspace.with_map_manager_mut(|manager| manager.snapshot());

        read_iter_text(buf, offset, areas.iter(), |area| {
            let perms = Self::perm_string(area.perm);
            let mut line = String::with_capacity(50);
            let _ = writeln!(line, "{:016x}-{:016x} {} {}", area.start, area.end, perms, area.name);
            Ok(line)
        })
    }

    fn writeat(&self, _buf: &[u8], _offset: usize) -> SysResult<usize> {
        Err(Errno::EROFS)
    }

    fn fstat(&self) -> SysResult<FileStat> {
        let mut kstat = FileStat::default();
        kstat.st_ino = self.get_ino() as u64;
        kstat.st_mode = self.mode()?.bits();
        kstat.st_nlink = 1;

        let tcb = manager::get(self.tid).ok_or(Errno::ESRCH)?;
        fill_kstat_common(&mut kstat, &tcb);

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
    tid: Tid,
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

    fn readat(&self, _buf: &mut [u8], _offset: usize, _direct: bool) -> SysResult<usize> {
        unreachable!()
    }

    fn writeat(&self, _buf: &[u8], _offset: usize) -> SysResult<usize> {
        unreachable!()
    }

    fn create(&self, _name: &str, _mode: Mode, _owner: Owner) -> SysResult<Arc<dyn InodeOps>> {
        Err(Errno::ENOTDIR)
    }

    fn readlink(&self, buf: &mut [u8]) -> SysResult<Option<usize>> {
        let tcb = manager::get(self.tid).ok_or(Errno::ESRCH)?;
        let exe_path = tcb.parent().exec_path();
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

        let tcb = manager::get(self.tid).ok_or(Errno::ESRCH)?;
        fill_kstat_common(&mut kstat, &tcb);

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

pub struct TaskStatInode {
    tid: Tid,
}

impl TaskStatInode {
    pub const INO_BASE: u32 = 0x400000;

    pub fn from_ino(ino: u32) -> Option<Self> {
        debug_assert!(ino >= Self::INO_BASE);
        let tid = (ino - Self::INO_BASE) as Tid;
        manager::get(tid)?;
        Some(Self { tid })
    }

    pub fn ino_from_tid(tid: Tid) -> u32 {
        Self::INO_BASE + tid as u32
    }

    fn state_char(state: TaskState, dead: bool) -> char {
        if dead {
            return 'Z';
        }
        match state {
            TaskState::Running | TaskState::Ready => 'R',
            TaskState::Blocked => 'S',
            TaskState::BlockedUninterruptible => 'D',
            TaskState::Exited => 'Z',
        }
    }
}

impl InodeOps for TaskStatInode {
    fn get_ino(&self) -> u32 {
        Self::ino_from_tid(self.tid)
    }

    fn type_name(&self) -> &'static str {
        "procfs_task_stat"
    }

    fn readat(&self, buf: &mut [u8], offset: usize, _direct: bool) -> SysResult<usize> {
        let tcb = manager::get(self.tid).ok_or(Errno::ESRCH)?;
        let pcb = tcb.parent();
        let pid = pcb.pid();
        let ppid = pcb.parent.lock().as_ref().map_or(0, |p| p.pid());
        let pgid = pcb.pgid();
        let exec_path = pcb.exec_path();
        let comm = exec_path.rsplit('/').next().unwrap_or(&exec_path);
        let state_set = tcb.state().lock();
        let state_char = Self::state_char(state_set.state(), state_set.is_dead());
        drop(state_set);

        let mut content = fixedstr::str96::new();
        let _ = write!(content, "{} ({}) {} {} {}\n", pid, comm, state_char, ppid, pgid);

        let content_bytes = content.as_bytes();
        if offset >= content_bytes.len() {
            return Ok(0);
        }
        let to_copy = min(buf.len(), content_bytes.len() - offset);
        buf[..to_copy].copy_from_slice(&content_bytes[offset..offset + to_copy]);
        Ok(to_copy)
    }

    fn writeat(&self, _buf: &[u8], _offset: usize) -> SysResult<usize> {
        Err(Errno::EROFS)
    }

    fn fstat(&self) -> SysResult<FileStat> {
        let mut kstat = FileStat::default();
        kstat.st_ino = self.get_ino() as u64;
        kstat.st_mode = self.mode()?.bits();
        kstat.st_nlink = 1;

        let tcb = manager::get(self.tid).ok_or(Errno::ESRCH)?;
        fill_kstat_common(&mut kstat, &tcb);

        Ok(kstat)
    }

    fn mode(&self) -> SysResult<Mode> {
        Ok(Mode::S_IFREG | Mode::S_IRUSR | Mode::S_IRGRP | Mode::S_IROTH)
    }

    fn size(&self) -> SysResult<u64> {
        Ok(0)
    }

    fn wrap_file(self: Arc<Self>, dentry: Option<Arc<Dentry>>, flags: FileFlags) -> Arc<dyn FileOps> {
        let dentry = dentry.expect("procfs stat requires associated dentry");
        Arc::new(File::new(self, dentry, flags))
    }
}

pub struct TaskStatusInode {
    tid: Tid,
}

impl TaskStatusInode {
    pub const INO_BASE: u32 = 0x500000;

    pub fn from_ino(ino: u32) -> Option<Self> {
        debug_assert!(ino >= Self::INO_BASE);
        let tid = (ino - Self::INO_BASE) as Tid;
        manager::get(tid)?;
        Some(Self { tid })
    }

    pub fn ino_from_tid(tid: Tid) -> u32 {
        Self::INO_BASE + tid as u32
    }

    fn state_desc(state: TaskState, dead: bool) -> &'static str {
        if dead {
            return "zombie";
        }
        match state {
            TaskState::Running | TaskState::Ready => "running",
            TaskState::Blocked => "sleeping",
            TaskState::BlockedUninterruptible => "disk sleep",
            TaskState::Exited => "zombie",
        }
    }
}

impl InodeOps for TaskStatusInode {
    fn get_ino(&self) -> u32 {
        Self::ino_from_tid(self.tid)
    }

    fn type_name(&self) -> &'static str {
        "procfs_task_status"
    }

    fn readat(&self, buf: &mut [u8], offset: usize, _direct: bool) -> SysResult<usize> {
        let tcb = manager::get(self.tid).ok_or(Errno::ESRCH)?;
        let pcb = tcb.parent();

        let pid = tcb.tid();
        let tgid = pcb.pid();
        let ppid = pcb.parent.lock().as_ref().map_or(0, |p| p.pid());
        let uid = pcb.uid();
        let euid = pcb.euid();
        let suid = pcb.suid();
        let fsuid = pcb.fsuid();
        let gid = pcb.gid();
        let egid = pcb.egid();
        let sgid = pcb.sgid();
        let fsgid = pcb.fsgid();
        let umask = pcb.umask();

        let exec_path = pcb.exec_path();
        let name = exec_path.rsplit('/').next().unwrap_or(&exec_path);

        let state_set = tcb.state().lock();
        let state_char = TaskStatInode::state_char(state_set.state(), state_set.is_dead());
        let state_desc = Self::state_desc(state_set.state(), state_set.is_dead());
        drop(state_set);

        let mut content = String::with_capacity(256);
        let _ = writeln!(content, "Name:\t{}", name);
        let _ = writeln!(content, "Umask:\t{:04o}", umask);
        let _ = writeln!(content, "State:\t{} ({})", state_char, state_desc);
        let _ = writeln!(content, "Tgid:\t{}", tgid);
        let _ = writeln!(content, "Pid:\t{}", pid);
        let _ = writeln!(content, "PPid:\t{}", ppid);
        let _ = writeln!(content, "Uid:\t{}\t{}\t{}\t{}", uid, euid, suid, fsuid);
        let _ = writeln!(content, "Gid:\t{}\t{}\t{}\t{}", gid, egid, sgid, fsgid);

        let content_bytes = content.as_bytes();
        if offset >= content_bytes.len() {
            return Ok(0);
        }
        let to_copy = min(buf.len(), content_bytes.len() - offset);
        buf[..to_copy].copy_from_slice(&content_bytes[offset..offset + to_copy]);
        Ok(to_copy)
    }

    fn writeat(&self, _buf: &[u8], _offset: usize) -> SysResult<usize> {
        Err(Errno::EROFS)
    }

    fn fstat(&self) -> SysResult<FileStat> {
        let mut kstat = FileStat::default();
        kstat.st_ino = self.get_ino() as u64;
        kstat.st_mode = self.mode()?.bits();
        kstat.st_nlink = 1;

        let tcb = manager::get(self.tid).ok_or(Errno::ESRCH)?;
        fill_kstat_common(&mut kstat, &tcb);

        Ok(kstat)
    }

    fn mode(&self) -> SysResult<Mode> {
        Ok(Mode::S_IFREG | Mode::S_IRUSR | Mode::S_IRGRP | Mode::S_IROTH)
    }

    fn size(&self) -> SysResult<u64> {
        Ok(0)
    }

    fn wrap_file(self: Arc<Self>, dentry: Option<Arc<Dentry>>, flags: FileFlags) -> Arc<dyn FileOps> {
        let dentry = dentry.expect("procfs status requires associated dentry");
        Arc::new(File::new(self, dentry, flags))
    }
}

pub struct TaskFdDirInode {
    tid: Tid,
}

impl TaskFdDirInode {
    pub const INO_BASE: u32 = 0x600000;

    pub fn from_ino(ino: u32) -> Option<Self> {
        debug_assert!(ino >= Self::INO_BASE);
        let tid = (ino - Self::INO_BASE) as Tid;
        manager::get(tid)?;
        Some(Self { tid })
    }

    pub fn ino_from_tid(tid: Tid) -> u32 {
        Self::INO_BASE + tid as u32
    }
}

impl InodeOps for TaskFdDirInode {
    fn get_ino(&self) -> u32 {
        Self::ino_from_tid(self.tid)
    }

    fn type_name(&self) -> &'static str {
        "procfs_task_fd_dir"
    }

    fn readat(&self, _buf: &mut [u8], _offset: usize, _direct: bool) -> SysResult<usize> {
        Err(Errno::EISDIR)
    }

    fn writeat(&self, _buf: &[u8], _offset: usize) -> SysResult<usize> {
        Err(Errno::EROFS)
    }

    fn lookup(&self, name: &str) -> SysResult<u32> {
        match name {
            "." => Ok(Self::ino_from_tid(self.tid)),
            ".." => Ok(TaskDirInode::ino_from_tid(self.tid)),
            _ => {
                let fd = name.parse::<usize>().map_err(|_| Errno::ENOENT)?;
                let tcb = manager::get(self.tid).ok_or(Errno::ESRCH)?;
                tcb.fdtable().lock().get(fd).map_err(|_| Errno::ENOENT)?;
                Ok(TaskFdEntryInode::ino_from_tid_fd(self.tid, fd))
            }
        }
    }

    fn get_dent(&self, index: usize) -> SysResult<Option<(DirResult, usize)>> {
        let tcb = manager::get(self.tid).ok_or(Errno::ESRCH)?;
        let fds = tcb.fdtable().lock().open_fds();

        let d = match index {
            0 => Some(DirResult {
                ino: Self::ino_from_tid(self.tid),
                name: ".".into(),
                file_type: FileType::Directory,
            }),
            1 => Some(DirResult {
                ino: TaskDirInode::ino_from_tid(self.tid),
                name: "..".into(),
                file_type: FileType::Directory,
            }),
            i => fds.get(i - 2).map(|fd| DirResult {
                ino: TaskFdEntryInode::ino_from_tid_fd(self.tid, *fd),
                name: fd.to_string(),
                file_type: FileType::Symlink,
            }),
        };

        Ok(d.map(|r| (r, index + 1)))
    }

    fn fstat(&self) -> SysResult<FileStat> {
        let mut kstat = FileStat::default();
        kstat.st_ino = self.get_ino() as u64;
        kstat.st_mode = self.mode()?.bits();
        kstat.st_nlink = 1;

        let tcb = manager::get(self.tid).ok_or(Errno::ESRCH)?;
        fill_kstat_common(&mut kstat, &tcb);

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
        let dentry = dentry.expect("procfs fd dir requires associated dentry");
        Arc::new(File::new(self, dentry, flags))
    }
}

pub struct TaskFdEntryInode {
    tid: Tid,
    fd: usize,
}

impl TaskFdEntryInode {
    pub const INO_BASE: u32 = 0x700000;

    pub fn from_ino(ino: u32) -> Option<Self> {
        debug_assert!(ino >= Self::INO_BASE);
        let offset = (ino - Self::INO_BASE) as usize;
        let tid = (offset / crate::kernel::config::MAX_FD) as Tid;
        let fd = offset % crate::kernel::config::MAX_FD;
        let tcb = manager::get(tid)?;
        tcb.fdtable().lock().get(fd).ok()?;

        Some(Self { tid, fd })
    }

    pub fn ino_from_tid_fd(tid: Tid, fd: usize) -> u32 {
        Self::INO_BASE + (tid as u32) * crate::kernel::config::MAX_FD as u32 + fd as u32
    }
}

impl InodeOps for TaskFdEntryInode {
    fn get_ino(&self) -> u32 {
        Self::ino_from_tid_fd(self.tid, self.fd)
    }

    fn type_name(&self) -> &'static str {
        "procfs_task_fd_entry"
    }

    fn readat(&self, _buf: &mut [u8], _offset: usize, _direct: bool) -> SysResult<usize> {
        Err(Errno::EINVAL)
    }

    fn writeat(&self, _buf: &[u8], _offset: usize) -> SysResult<usize> {
        Err(Errno::EROFS)
    }

    fn readlink(&self, buf: &mut [u8]) -> SysResult<Option<usize>> {
        let tcb = manager::get(self.tid).ok_or(Errno::ESRCH)?;
        let file = tcb.fdtable().lock().get(self.fd).map_err(|_| Errno::ENOENT)?;
        let target = if let Some(dentry) = file.get_dentry() {
            dentry.get_path()
        } else {
            let mut s = String::from("anon_inode:[");
            s.push_str(file.type_name());
            s.push(']');
            s
        };

        let bytes = target.as_bytes();
        let to_copy = min(buf.len(), bytes.len());
        buf[..to_copy].copy_from_slice(&bytes[..to_copy]);
        Ok(Some(to_copy))
    }

    fn fstat(&self) -> SysResult<FileStat> {
        let mut kstat = FileStat::default();
        kstat.st_ino = self.get_ino() as u64;
        kstat.st_mode = self.mode()?.bits();
        kstat.st_nlink = 1;

        let tcb = manager::get(self.tid).ok_or(Errno::ESRCH)?;
        fill_kstat_common(&mut kstat, &tcb);

        Ok(kstat)
    }

    fn mode(&self) -> SysResult<Mode> {
        Ok(Mode::S_IFLNK
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

    fn owner(&self) -> SysResult<(Uid, Uid)> {
        Ok((0, 0))
    }

    fn wrap_file(self: Arc<Self>, dentry: Option<Arc<Dentry>>, flags: FileFlags) -> Arc<dyn FileOps> {
        Arc::new(File::new(self, dentry.unwrap(), flags))
    }
}
