use alloc::string::{String, ToString};
use alloc::sync::Arc;
use alloc::vec::Vec;
use core::cmp::min;
use core::fmt::Write;

use crate::fs::file::{DirResult, FileFlags, FileOps, RandomAccessFile};
use crate::fs::procfs::inode::{fill_kstat_common, read_iter_text};
use crate::fs::{Dentry, FileType, Inode, InodeOps, Mode, Owner};
use crate::kernel::errno::{Errno, SysResult};
use crate::kernel::mm::MapPerm;
use crate::kernel::scheduler::Tid;
use crate::kernel::scheduler::tid::PID_MAX;
use crate::kernel::task::pidfd::PidFile;
use crate::kernel::task::{TCBState, manager};
use crate::kernel::uapi::{FileStat, Uid};

use super::RootInode;

const CLK_TCK: u64 = 100;

fn process_leader_tid(tid: Tid) -> SysResult<Tid> {
    let tcb = manager::get(tid).ok_or(Errno::ESRCH)?;
    Ok(tcb.parent().pid())
}

fn duration_to_clock_ticks(duration: core::time::Duration) -> usize {
    (duration.as_millis() as u64 * CLK_TCK / 1000) as usize
}

fn thread_group_tids(pid: Tid) -> SysResult<Vec<Tid>> {
    let tcb = manager::get(pid).ok_or(Errno::ESRCH)?;
    let pcb = tcb.parent();
    if pcb.pid() != pid {
        return Err(Errno::ENOENT);
    }

    let mut tids = pcb.tasks.lock().iter().map(|task| task.tid()).collect::<Vec<_>>();
    tids.sort_unstable();
    Ok(tids)
}

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
            "task" if process_leader_tid(self.tid)? == self.tid => Ok(TaskTaskDirInode::ino_from_pid(self.tid)),
            "maps" => Ok(TaskMapsInode::ino_from_tid(self.tid)),
            "exe" => Ok(TaskExeInode::ino_from_tid(self.tid)),
            "stat" => Ok(TaskStatInode::ino_from_tid(self.tid)),
            "status" => Ok(TaskStatusInode::ino_from_tid(self.tid)),
            "fd" => Ok(TaskFdDirInode::ino_from_tid(self.tid)),
            "fdinfo" => Ok(TaskFdInfoDirInode::ino_from_tid(self.tid)),
            _ => Err(Errno::ENOENT),
        }
    }

    fn get_dent(&self, index: usize) -> SysResult<Option<(DirResult, usize)>> {
        let has_task_dir = process_leader_tid(self.tid)? == self.tid;
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
            7 => Some(DirResult {
                ino: TaskFdInfoDirInode::ino_from_tid(self.tid),
                name: "fdinfo".into(),
                file_type: FileType::Directory,
            }),
            8 if has_task_dir => Some(DirResult {
                ino: TaskTaskDirInode::ino_from_pid(self.tid),
                name: "task".into(),
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

    fn wrap_file(&self, inode: Arc<Inode>, dentry: Option<Arc<Dentry>>, flags: FileFlags) -> Arc<dyn FileOps> {
        let dentry = dentry.expect("procfs task dir requires associated dentry");
        if let Some(tcb) = manager::get(self.tid)
            && tcb.parent().pid() == self.tid
        {
            let file = Arc::new(RandomAccessFile::new(inode.clone(), dentry, flags));
            return Arc::new(PidFile::new_with_file(tcb.parent(), file, flags));
        }

        Arc::new(RandomAccessFile::new(inode, dentry, flags))
    }
}

pub struct TaskTaskDirInode {
    pid: Tid,
}

impl TaskTaskDirInode {
    pub const INO_BASE: u32 = 0x5000_0000;
    pub const INO_END: u32 = Self::INO_BASE + PID_MAX as u32;

    pub fn from_ino(ino: u32) -> Option<Self> {
        debug_assert!(ino >= Self::INO_BASE);
        let pid = (ino - Self::INO_BASE) as Tid;
        let tcb = manager::get(pid)?;
        if tcb.parent().pid() != pid {
            return None;
        }
        Some(Self { pid })
    }

    pub fn ino_from_pid(pid: Tid) -> u32 {
        Self::INO_BASE + pid as u32
    }
}

impl InodeOps for TaskTaskDirInode {
    fn get_ino(&self) -> u32 {
        Self::ino_from_pid(self.pid)
    }

    fn type_name(&self) -> &'static str {
        "procfs_task_task_dir"
    }

    fn readat(&self, _buf: &mut [u8], _offset: usize, _direct: bool) -> SysResult<usize> {
        Err(Errno::EISDIR)
    }

    fn writeat(&self, _buf: &[u8], _offset: usize) -> SysResult<usize> {
        Err(Errno::EROFS)
    }

    fn lookup(&self, name: &str) -> SysResult<u32> {
        match name {
            "." => Ok(Self::ino_from_pid(self.pid)),
            ".." => Ok(TaskDirInode::ino_from_tid(self.pid)),
            _ => {
                let tid = name.parse::<Tid>().map_err(|_| Errno::ENOENT)?;
                let tcb = manager::get(tid).ok_or(Errno::ENOENT)?;
                if tcb.parent().pid() != self.pid {
                    return Err(Errno::ENOENT);
                }
                Ok(TaskThreadDirInode::ino_from_tid(tid))
            }
        }
    }

    fn get_dent(&self, index: usize) -> SysResult<Option<(DirResult, usize)>> {
        let tids = thread_group_tids(self.pid)?;
        let d = match index {
            0 => Some(DirResult {
                ino: Self::ino_from_pid(self.pid),
                name: ".".into(),
                file_type: FileType::Directory,
            }),
            1 => Some(DirResult {
                ino: TaskDirInode::ino_from_tid(self.pid),
                name: "..".into(),
                file_type: FileType::Directory,
            }),
            i => tids.get(i - 2).map(|tid| DirResult {
                ino: TaskThreadDirInode::ino_from_tid(*tid),
                name: tid.to_string(),
                file_type: FileType::Directory,
            }),
        };

        Ok(d.map(|r| (r, index + 1)))
    }

    fn fstat(&self) -> SysResult<FileStat> {
        let mut kstat = FileStat::default();
        kstat.st_ino = self.get_ino() as u64;
        kstat.st_mode = self.mode()?.bits();
        kstat.st_nlink = 1;

        let tcb = manager::get(self.pid).ok_or(Errno::ESRCH)?;
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

    fn wrap_file(&self, inode: Arc<Inode>, dentry: Option<Arc<Dentry>>, flags: FileFlags) -> Arc<dyn FileOps> {
        let dentry = dentry.expect("procfs task/task dir requires associated dentry");
        Arc::new(RandomAccessFile::new(inode, dentry, flags))
    }
}

pub struct TaskThreadDirInode {
    tid: Tid,
}

impl TaskThreadDirInode {
    pub const INO_BASE: u32 = 0x5010_0000;
    pub const INO_END: u32 = Self::INO_BASE + PID_MAX as u32;

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

impl InodeOps for TaskThreadDirInode {
    fn get_ino(&self) -> u32 {
        Self::ino_from_tid(self.tid)
    }

    fn type_name(&self) -> &'static str {
        "procfs_task_thread_dir"
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
            ".." => Ok(TaskTaskDirInode::ino_from_pid(process_leader_tid(self.tid)?)),
            "maps" => Ok(TaskMapsInode::ino_from_tid(self.tid)),
            "exe" => Ok(TaskExeInode::ino_from_tid(self.tid)),
            "stat" => Ok(TaskStatInode::ino_from_tid(self.tid)),
            "status" => Ok(TaskStatusInode::ino_from_tid(self.tid)),
            "fd" => Ok(TaskFdDirInode::ino_from_tid(self.tid)),
            "fdinfo" => Ok(TaskFdInfoDirInode::ino_from_tid(self.tid)),
            _ => Err(Errno::ENOENT),
        }
    }

    fn get_dent(&self, index: usize) -> SysResult<Option<(DirResult, usize)>> {
        let parent_ino = TaskTaskDirInode::ino_from_pid(process_leader_tid(self.tid)?);
        let d = match index {
            0 => Some(DirResult {
                ino: Self::ino_from_tid(self.tid),
                name: ".".into(),
                file_type: FileType::Directory,
            }),
            1 => Some(DirResult {
                ino: parent_ino,
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
            7 => Some(DirResult {
                ino: TaskFdInfoDirInode::ino_from_tid(self.tid),
                name: "fdinfo".into(),
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

    fn wrap_file(&self, inode: Arc<Inode>, dentry: Option<Arc<Dentry>>, flags: FileFlags) -> Arc<dyn FileOps> {
        let dentry = dentry.expect("procfs task/thread dir requires associated dentry");
        Arc::new(RandomAccessFile::new(inode, dentry, flags))
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

    fn perm_string(perm: MapPerm, shared: bool) -> String {
        let mut perms = String::with_capacity(4);
        perms.push(if perm.contains(MapPerm::R) { 'r' } else { '-' });
        perms.push(if perm.contains(MapPerm::W) { 'w' } else { '-' });
        perms.push(if perm.contains(MapPerm::X) { 'x' } else { '-' });
        perms.push(if shared { 's' } else { 'p' });
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
        let areas = addrspace.with_map_manager(|manager| manager.snapshot());

        read_iter_text(buf, offset, areas.iter(), |area| {
            let perms = Self::perm_string(area.perm, area.shared);
            let path_len = area.path.as_ref().map_or(0, |path| path.len() + 1);
            let mut line = String::with_capacity(80 + path_len);
            let _ = write!(
                line,
                "{:x}-{:x} {} {:x} {:x}:{:x} {:>5}",
                area.start, area.end, perms, area.offset, area.dev_major, area.dev_minor, area.inode
            );
            if let Some(path) = &area.path {
                let _ = write!(line, " {}", path);
            }
            line.push('\n');
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

    fn wrap_file(&self, inode: Arc<Inode>, dentry: Option<Arc<Dentry>>, flags: FileFlags) -> Arc<dyn FileOps> {
        let dentry = dentry.expect("procfs maps requires associated dentry");
        Arc::new(RandomAccessFile::new(inode, dentry, flags))
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

    fn create(&self, _name: &str, _mode: Mode, _owner: Owner) -> SysResult<Self> {
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

    fn wrap_file(&self, inode: Arc<Inode>, dentry: Option<Arc<Dentry>>, flags: FileFlags) -> Arc<dyn FileOps> {
        Arc::new(RandomAccessFile::new(inode, dentry.unwrap(), flags))
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

    fn state_char(state: TCBState, dead: bool) -> char {
        if dead {
            return 'Z';
        }
        match state {
            TCBState::Running | TCBState::Ready => 'R',
            TCBState::Blocked => 'S',
            TCBState::BlockedUninterruptible => 'D',
            TCBState::Stopped => 'T',
            TCBState::PtraceStop(_) => 't',
            TCBState::Exited => 'Z',
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
        let sid = pcb.sid();
        let exec_path = pcb.exec_path();
        let comm = exec_path.rsplit('/').next().unwrap_or(&exec_path);
        let state_set = tcb.state().lock();
        let state_char = Self::state_char(state_set.state(), state_set.is_dead());
        drop(state_set);
        let (utime, stime) = pcb.tasks_usage_time();
        let utime = duration_to_clock_ticks(utime);
        let stime = duration_to_clock_ticks(stime);

        let mut content = String::with_capacity(128);
        let _ = writeln!(
            content,
            "{} ({}) {} {} {} {} 0 0 0 0 0 0 0 {} {}",
            pid, comm, state_char, ppid, pgid, sid, utime, stime
        );

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

    fn wrap_file(&self, inode: Arc<Inode>, dentry: Option<Arc<Dentry>>, flags: FileFlags) -> Arc<dyn FileOps> {
        let dentry = dentry.expect("procfs stat requires associated dentry");
        Arc::new(RandomAccessFile::new(inode, dentry, flags))
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

    fn state_desc(state: TCBState, dead: bool) -> &'static str {
        if dead {
            return "zombie";
        }
        match state {
            TCBState::Running | TCBState::Ready => "running",
            TCBState::Blocked => "sleeping",
            TCBState::BlockedUninterruptible => "disk sleep",
            TCBState::Stopped => "stopped",
            TCBState::PtraceStop(_) => "tracing stop",
            TCBState::Exited => "zombie",
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

    fn wrap_file(&self, inode: Arc<Inode>, dentry: Option<Arc<Dentry>>, flags: FileFlags) -> Arc<dyn FileOps> {
        let dentry = dentry.expect("procfs status requires associated dentry");
        Arc::new(RandomAccessFile::new(inode, dentry, flags))
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

    fn wrap_file(&self, inode: Arc<Inode>, dentry: Option<Arc<Dentry>>, flags: FileFlags) -> Arc<dyn FileOps> {
        let dentry = dentry.expect("procfs fd dir requires associated dentry");
        Arc::new(RandomAccessFile::new(inode, dentry, flags))
    }
}

pub struct TaskFdEntryInode {
    tid: Tid,
    fd: usize,
}

impl TaskFdEntryInode {
    pub const INO_BASE: u32 = 0x700000;
    pub const INO_END: u32 = Self::INO_BASE + (PID_MAX as u32) * crate::kernel::config::MAX_FD as u32;

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
        let file = {
            let fdtable = tcb.fdtable();
            let mut fdtable = fdtable.lock();
            fdtable.get(self.fd).map_err(|_| Errno::ENOENT)?
        };
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

    fn follow_magic_link(&self) -> SysResult<Option<Arc<Dentry>>> {
        let tcb = manager::get(self.tid).ok_or(Errno::ESRCH)?;
        let file = {
            let fdtable = tcb.fdtable();
            let mut fdtable = fdtable.lock();
            fdtable.get(self.fd).map_err(|_| Errno::ENOENT)?
        };
        Ok(file.get_dentry().cloned())
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

    fn wrap_file(&self, inode: Arc<Inode>, dentry: Option<Arc<Dentry>>, flags: FileFlags) -> Arc<dyn FileOps> {
        Arc::new(RandomAccessFile::new(inode, dentry.unwrap(), flags))
    }
}

pub struct TaskFdInfoDirInode {
    tid: Tid,
}

impl TaskFdInfoDirInode {
    pub const INO_BASE: u32 = 0x5020_0000;
    pub const INO_END: u32 = Self::INO_BASE + PID_MAX as u32;

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

impl InodeOps for TaskFdInfoDirInode {
    fn get_ino(&self) -> u32 {
        Self::ino_from_tid(self.tid)
    }

    fn type_name(&self) -> &'static str {
        "procfs_task_fdinfo_dir"
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
                Ok(TaskFdInfoEntryInode::ino_from_tid_fd(self.tid, fd))
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
                ino: TaskFdInfoEntryInode::ino_from_tid_fd(self.tid, *fd),
                name: fd.to_string(),
                file_type: FileType::Regular,
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

    fn wrap_file(&self, inode: Arc<Inode>, dentry: Option<Arc<Dentry>>, flags: FileFlags) -> Arc<dyn FileOps> {
        let dentry = dentry.expect("procfs fdinfo dir requires associated dentry");
        Arc::new(RandomAccessFile::new(inode, dentry, flags))
    }
}

pub struct TaskFdInfoEntryInode {
    tid: Tid,
    fd: usize,
}

impl TaskFdInfoEntryInode {
    pub const INO_BASE: u32 = 0x8000_0000;
    pub const INO_END: u32 = Self::INO_BASE + (PID_MAX as u32) * crate::kernel::config::MAX_FD as u32;

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

impl InodeOps for TaskFdInfoEntryInode {
    fn get_ino(&self) -> u32 {
        Self::ino_from_tid_fd(self.tid, self.fd)
    }

    fn type_name(&self) -> &'static str {
        "procfs_task_fdinfo_entry"
    }

    fn readat(&self, buf: &mut [u8], offset: usize, _direct: bool) -> SysResult<usize> {
        let tcb = manager::get(self.tid).ok_or(Errno::ESRCH)?;
        let file = tcb.fdtable().lock().get(self.fd).map_err(|_| Errno::ENOENT)?;
        let content = file.fdinfo().unwrap_or_default();
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

    fn wrap_file(&self, inode: Arc<Inode>, dentry: Option<Arc<Dentry>>, flags: FileFlags) -> Arc<dyn FileOps> {
        let dentry = dentry.expect("procfs fdinfo entry requires associated dentry");
        Arc::new(RandomAccessFile::new(inode, dentry, flags))
    }
}
