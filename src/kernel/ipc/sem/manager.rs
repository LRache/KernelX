use alloc::collections::BTreeMap;
use alloc::vec::Vec;
use bitflags::bitflags;

use crate::kernel::errno::{Errno, SysResult};
use crate::kernel::event::{Event, WaitQueue};
use crate::kernel::ipc::{IPC_PRIVATE, IpcGetFlag, IpcMode};
use crate::kernel::scheduler::Tid;
use crate::kernel::uapi::Uid;
use crate::klib::SpinLock;

pub mod limit {
    pub const MAX_SETS: usize = 32000;
    pub const MAX_SEMAPHORES_PER_SET: usize = 32000;
    pub const MAX_SEMAPHORES: usize = MAX_SETS * MAX_SEMAPHORES_PER_SET;
    pub const MAP_ENTRIES: usize = MAX_SEMAPHORES;
    pub const MAX_UNDO_STRUCTURES: usize = MAX_SEMAPHORES;
    pub const MAX_OPS_PER_CALL: usize = 500;
    pub const MAX_UNDO_ENTRIES_PER_PROCESS: usize = MAX_OPS_PER_CALL;
    pub const SEM_INFO_SIZE: usize = 20;
    pub const MAX_VALUE: i32 = 32767;
    pub const MAX_ADJUST_ON_EXIT: i32 = MAX_VALUE;

    pub fn usize_to_i32(value: usize) -> i32 {
        value.min(i32::MAX as usize) as i32
    }
}

bitflags! {
    struct SemOpFlags: u16 {
        const IPC_NOWAIT = 0o4000;
        const SEM_UNDO = 0x1000;
    }
}

#[derive(Clone, Copy, Debug)]
pub struct SemOp {
    pub sem_num: u16,
    pub sem_op: i16,
    pub sem_flg: i16,
}

#[derive(Clone, Copy, Debug)]
pub struct SemidDs {
    pub key: usize,
    pub uid: Uid,
    pub gid: Uid,
    pub cuid: Uid,
    pub cgid: Uid,
    pub mode: u32,
    pub otime: usize,
    pub ctime: usize,
    pub nsems: usize,
}

#[derive(Clone, Copy, Debug)]
pub struct SemStat {
    pub ds: SemidDs,
}

#[derive(Clone, Copy, Debug)]
pub struct SemInfo {
    pub semmap: i32,
    pub semmni: i32,
    pub semmns: i32,
    pub semmnu: i32,
    pub semmsl: i32,
    pub semopm: i32,
    pub semume: i32,
    pub semusz: i32,
    pub semvmx: i32,
    pub semaem: i32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SemopStatus {
    Done,
    WouldBlock,
}

#[derive(Clone, Copy, Debug)]
struct Semaphore {
    value: i32,
    pid: Tid,
}

impl Semaphore {
    const fn new() -> Self {
        Self { value: 0, pid: 0 }
    }
}

#[derive(Clone, Copy, Debug)]
struct SemWait {
    sem_num: u16,
    wait_zero: bool,
}

struct SemIdentifier {
    ds: SemidDs,
    sems: Vec<Semaphore>,
    waiters: WaitQueue<SemWait>,
    deleted: bool,
}

pub struct SemManager {
    sems: BTreeMap<usize, SemIdentifier>,
    next_semid: usize,
}

fn has_sem_perm(sem: &SemIdentifier, uid: Uid, gid: Uid, write: bool) -> bool {
    if uid == 0 {
        return true;
    }

    let required = if write { IpcMode::WRITE } else { IpcMode::READ };
    let shift = if uid == sem.ds.uid {
        6
    } else if gid == sem.ds.gid {
        3
    } else {
        0
    };
    let allowed = IpcMode::from_bits_truncate((sem.ds.mode >> shift) & IpcMode::ALL.bits());

    allowed.contains(required)
}

fn has_sem_get_perm(sem: &SemIdentifier, mode: u32, uid: Uid, gid: Uid) -> bool {
    if uid == 0 {
        return true;
    }

    let access_mask = IpcMode::ALL.bits() >> 6;
    let requested = ((mode >> 6) | (mode >> 3) | mode) & access_mask;
    let shift = if uid == sem.ds.uid {
        6
    } else if gid == sem.ds.gid {
        3
    } else {
        0
    };
    let granted = (sem.ds.mode >> shift) & access_mask;

    requested & !granted == 0
}

impl SemManager {
    const fn new() -> Self {
        Self {
            sems: BTreeMap::new(),
            next_semid: 1,
        }
    }

    fn get_or_create(
        &mut self,
        key: usize,
        nsems: usize,
        flags: IpcGetFlag,
        mode: u32,
        uid: Uid,
        gid: Uid,
    ) -> SysResult<usize> {
        if key != IPC_PRIVATE {
            let existing = self
                .sems
                .iter()
                .find(|(_, sem)| !sem.deleted && sem.ds.key == key)
                .map(|(id, _)| *id);

            if let Some(id) = existing {
                if flags.contains(IpcGetFlag::IPC_CREAT | IpcGetFlag::IPC_EXCL) {
                    return Err(Errno::EEXIST);
                }
                let sem = self.sems.get(&id).unwrap();
                if nsems != 0 && nsems > sem.ds.nsems {
                    return Err(Errno::EINVAL);
                }
                if !has_sem_get_perm(sem, mode, uid, gid) {
                    return Err(Errno::EACCES);
                }
                return Ok(id);
            }
        }

        if key != IPC_PRIVATE && !flags.contains(IpcGetFlag::IPC_CREAT) {
            return Err(Errno::ENOENT);
        }
        if nsems == 0 || nsems > limit::MAX_SEMAPHORES_PER_SET {
            return Err(Errno::EINVAL);
        }

        let id = self.next_semid;
        self.next_semid = self.next_semid.checked_add(1).ok_or(Errno::ENOSPC)?;

        self.sems.insert(
            id,
            SemIdentifier {
                ds: SemidDs {
                    key,
                    uid,
                    gid,
                    cuid: uid,
                    cgid: gid,
                    mode,
                    otime: 0,
                    ctime: 0,
                    nsems,
                },
                sems: alloc::vec![Semaphore::new(); nsems],
                waiters: WaitQueue::new(),
                deleted: false,
            },
        );

        Ok(id)
    }

    fn stat(&self, semid: usize, uid: Uid, gid: Uid) -> SysResult<SemStat> {
        let sem = self.sems.get(&semid).ok_or(Errno::EINVAL)?;
        if sem.deleted {
            return Err(Errno::EIDRM);
        }
        if !has_sem_perm(sem, uid, gid, false) {
            return Err(Errno::EACCES);
        }
        Ok(SemStat { ds: sem.ds })
    }

    fn stat_index(&self, index: usize, uid: Uid, gid: Uid) -> SysResult<(usize, SemStat)> {
        let sem = self.sems.get(&index).ok_or(Errno::EINVAL)?;
        if sem.deleted {
            return Err(Errno::EINVAL);
        }
        if !has_sem_perm(sem, uid, gid, false) {
            return Err(Errno::EACCES);
        }
        Ok((index, SemStat { ds: sem.ds }))
    }

    fn info(&self, usage: bool) -> (usize, SemInfo) {
        let mut highest_index = 0;
        let mut set_count = 0;
        let mut sem_count = 0;

        for (id, sem) in &self.sems {
            if sem.deleted {
                continue;
            }
            highest_index = highest_index.max(*id);
            set_count += 1;
            sem_count += sem.ds.nsems;
        }

        let semusz = if usage {
            limit::usize_to_i32(set_count)
        } else {
            limit::usize_to_i32(limit::SEM_INFO_SIZE)
        };
        let semaem = if usage {
            limit::usize_to_i32(sem_count)
        } else {
            limit::MAX_ADJUST_ON_EXIT
        };

        (
            highest_index,
            SemInfo {
                semmap: limit::usize_to_i32(limit::MAP_ENTRIES),
                semmni: limit::usize_to_i32(limit::MAX_SETS),
                semmns: limit::usize_to_i32(limit::MAX_SEMAPHORES),
                semmnu: limit::usize_to_i32(limit::MAX_UNDO_STRUCTURES),
                semmsl: limit::usize_to_i32(limit::MAX_SEMAPHORES_PER_SET),
                semopm: limit::usize_to_i32(limit::MAX_OPS_PER_CALL),
                semume: limit::usize_to_i32(limit::MAX_UNDO_ENTRIES_PER_PROCESS),
                semusz,
                semvmx: limit::MAX_VALUE,
                semaem,
            },
        )
    }

    fn set_perm(&mut self, semid: usize, uid: Uid, gid: Uid, mode: u32, caller_uid: Uid) -> SysResult<()> {
        let sem = self.sems.get_mut(&semid).ok_or(Errno::EINVAL)?;
        if sem.deleted {
            return Err(Errno::EIDRM);
        }
        if caller_uid != 0 && caller_uid != sem.ds.uid && caller_uid != sem.ds.cuid {
            return Err(Errno::EPERM);
        }

        sem.ds.uid = uid;
        sem.ds.gid = gid;
        sem.ds.mode = mode & IpcMode::ALL.bits();
        sem.ds.ctime = 0;
        Ok(())
    }

    fn nsems(&self, semid: usize) -> SysResult<usize> {
        let sem = self.sems.get(&semid).ok_or(Errno::EINVAL)?;
        if sem.deleted {
            return Err(Errno::EIDRM);
        }
        Ok(sem.ds.nsems)
    }

    fn getval(&self, semid: usize, semnum: usize, uid: Uid, gid: Uid) -> SysResult<usize> {
        let sem = self.sems.get(&semid).ok_or(Errno::EINVAL)?;
        if sem.deleted {
            return Err(Errno::EIDRM);
        }
        if !has_sem_perm(sem, uid, gid, false) {
            return Err(Errno::EACCES);
        }
        let item = sem.sems.get(semnum).ok_or(Errno::EINVAL)?;
        Ok(item.value as usize)
    }

    fn getpid(&self, semid: usize, semnum: usize, uid: Uid, gid: Uid) -> SysResult<usize> {
        let sem = self.sems.get(&semid).ok_or(Errno::EINVAL)?;
        if sem.deleted {
            return Err(Errno::EIDRM);
        }
        if !has_sem_perm(sem, uid, gid, false) {
            return Err(Errno::EACCES);
        }
        let item = sem.sems.get(semnum).ok_or(Errno::EINVAL)?;
        Ok(item.pid as usize)
    }

    fn get_wait_count(&self, semid: usize, semnum: usize, wait_zero: bool, uid: Uid, gid: Uid) -> SysResult<usize> {
        let sem = self.sems.get(&semid).ok_or(Errno::EINVAL)?;
        if sem.deleted {
            return Err(Errno::EIDRM);
        }
        if !has_sem_perm(sem, uid, gid, false) {
            return Err(Errno::EACCES);
        }
        if semnum >= sem.sems.len() {
            return Err(Errno::EINVAL);
        }
        Ok(sem
            .waiters
            .count_by(|wait| usize::from(wait.sem_num) == semnum && wait.wait_zero == wait_zero))
    }

    fn getall(&self, semid: usize, uid: Uid, gid: Uid) -> SysResult<Vec<u16>> {
        let sem = self.sems.get(&semid).ok_or(Errno::EINVAL)?;
        if sem.deleted {
            return Err(Errno::EIDRM);
        }
        if !has_sem_perm(sem, uid, gid, false) {
            return Err(Errno::EACCES);
        }
        Ok(sem.sems.iter().map(|item| item.value as u16).collect())
    }

    fn setval(&mut self, semid: usize, semnum: usize, value: i32, uid: Uid, gid: Uid, pid: Tid) -> SysResult<()> {
        if !(0..=limit::MAX_VALUE).contains(&value) {
            return Err(Errno::ERANGE);
        }

        let sem = self.sems.get_mut(&semid).ok_or(Errno::EINVAL)?;
        if sem.deleted {
            return Err(Errno::EIDRM);
        }
        if !has_sem_perm(sem, uid, gid, true) {
            return Err(Errno::EACCES);
        }

        let item = sem.sems.get_mut(semnum).ok_or(Errno::EINVAL)?;
        item.value = value;
        item.pid = pid;
        sem.ds.ctime = 0;
        sem.waiters.wake_all(|_| Event::Sem);
        Ok(())
    }

    fn setall(&mut self, semid: usize, values: &[u16], uid: Uid, gid: Uid, pid: Tid) -> SysResult<()> {
        if values.iter().any(|&value| i32::from(value) > limit::MAX_VALUE) {
            return Err(Errno::ERANGE);
        }

        let sem = self.sems.get_mut(&semid).ok_or(Errno::EINVAL)?;
        if sem.deleted {
            return Err(Errno::EIDRM);
        }
        if values.len() != sem.ds.nsems {
            return Err(Errno::EINVAL);
        }
        if !has_sem_perm(sem, uid, gid, true) {
            return Err(Errno::EACCES);
        }

        for (item, &value) in sem.sems.iter_mut().zip(values) {
            item.value = i32::from(value);
            item.pid = pid;
        }
        sem.ds.ctime = 0;
        sem.waiters.wake_all(|_| Event::Sem);
        Ok(())
    }

    fn remove(&mut self, semid: usize, uid: Uid) -> SysResult<()> {
        let sem = self.sems.get_mut(&semid).ok_or(Errno::EINVAL)?;
        if uid != 0 && uid != sem.ds.uid && uid != sem.ds.cuid {
            return Err(Errno::EPERM);
        }
        sem.deleted = true;
        sem.waiters.wake_all(|_| Event::Sem);
        Ok(())
    }

    fn begin_semop(&mut self, semid: usize, ops: &[SemOp], uid: Uid, gid: Uid, pid: Tid) -> SysResult<SemopStatus> {
        if ops.is_empty() {
            return Err(Errno::EINVAL);
        }
        if ops.len() > limit::MAX_OPS_PER_CALL {
            return Err(Errno::E2BIG);
        }

        let sem = self.sems.get_mut(&semid).ok_or(Errno::EINVAL)?;
        if sem.deleted {
            return Err(Errno::EIDRM);
        }

        let write = ops.iter().any(|op| op.sem_op != 0);
        if !has_sem_perm(sem, uid, gid, write) {
            return Err(Errno::EACCES);
        }

        for op in ops {
            SemOpFlags::from_bits(op.sem_flg as u16).ok_or(Errno::EINVAL)?;
            let index = usize::from(op.sem_num);
            if index >= sem.sems.len() {
                return Err(Errno::EFBIG);
            }
        }

        let mut values: Vec<i32> = sem.sems.iter().map(|item| item.value).collect();
        for op in ops {
            let flags = SemOpFlags::from_bits(op.sem_flg as u16).ok_or(Errno::EINVAL)?;
            let value = &mut values[usize::from(op.sem_num)];
            match op.sem_op {
                1..=i16::MAX => {
                    *value = value.checked_add(i32::from(op.sem_op)).ok_or(Errno::ERANGE)?;
                    if *value > limit::MAX_VALUE {
                        return Err(Errno::ERANGE);
                    }
                }
                0 => {
                    if *value != 0 {
                        if flags.contains(SemOpFlags::IPC_NOWAIT) {
                            return Err(Errno::EAGAIN);
                        }
                        sem.waiters.wait_current(SemWait {
                            sem_num: op.sem_num,
                            wait_zero: true,
                        });
                        return Ok(SemopStatus::WouldBlock);
                    }
                }
                i16::MIN..=-1 => {
                    let decrement = -i32::from(op.sem_op);
                    if *value < decrement {
                        if flags.contains(SemOpFlags::IPC_NOWAIT) {
                            return Err(Errno::EAGAIN);
                        }
                        sem.waiters.wait_current(SemWait {
                            sem_num: op.sem_num,
                            wait_zero: false,
                        });
                        return Ok(SemopStatus::WouldBlock);
                    }
                    *value -= decrement;
                }
            }
        }

        for (item, value) in sem.sems.iter_mut().zip(values) {
            item.value = value;
        }
        for op in ops {
            sem.sems[usize::from(op.sem_num)].pid = pid;
        }
        sem.ds.otime = 0;
        sem.waiters.wake_all(|_| Event::Sem);
        Ok(SemopStatus::Done)
    }

    fn remove_current_waiter(&mut self, semid: usize) {
        if let Some(sem) = self.sems.get_mut(&semid) {
            sem.waiters.remove_current();
        }
    }
}

static SEM_MANAGER: SpinLock<SemManager> = SpinLock::new(SemManager::new(), "static::SEM_MANAGER");

pub fn get_or_create_sem(
    key: usize,
    nsems: usize,
    flags: IpcGetFlag,
    mode: u32,
    uid: Uid,
    gid: Uid,
) -> SysResult<usize> {
    SEM_MANAGER.lock().get_or_create(key, nsems, flags, mode, uid, gid)
}

pub fn stat_sem(semid: usize, uid: Uid, gid: Uid) -> SysResult<SemStat> {
    SEM_MANAGER.lock().stat(semid, uid, gid)
}

pub fn stat_index_sem(index: usize, uid: Uid, gid: Uid) -> SysResult<(usize, SemStat)> {
    SEM_MANAGER.lock().stat_index(index, uid, gid)
}

pub fn info_sem(usage: bool) -> (usize, SemInfo) {
    SEM_MANAGER.lock().info(usage)
}

pub fn set_perm_sem(semid: usize, uid: Uid, gid: Uid, mode: u32, caller_uid: Uid) -> SysResult<()> {
    SEM_MANAGER.lock().set_perm(semid, uid, gid, mode, caller_uid)
}

pub fn nsems(semid: usize) -> SysResult<usize> {
    SEM_MANAGER.lock().nsems(semid)
}

pub fn getval(semid: usize, semnum: usize, uid: Uid, gid: Uid) -> SysResult<usize> {
    SEM_MANAGER.lock().getval(semid, semnum, uid, gid)
}

pub fn getpid(semid: usize, semnum: usize, uid: Uid, gid: Uid) -> SysResult<usize> {
    SEM_MANAGER.lock().getpid(semid, semnum, uid, gid)
}

pub fn getncnt(semid: usize, semnum: usize, uid: Uid, gid: Uid) -> SysResult<usize> {
    SEM_MANAGER.lock().get_wait_count(semid, semnum, false, uid, gid)
}

pub fn getzcnt(semid: usize, semnum: usize, uid: Uid, gid: Uid) -> SysResult<usize> {
    SEM_MANAGER.lock().get_wait_count(semid, semnum, true, uid, gid)
}

pub fn getall(semid: usize, uid: Uid, gid: Uid) -> SysResult<Vec<u16>> {
    SEM_MANAGER.lock().getall(semid, uid, gid)
}

pub fn setval(semid: usize, semnum: usize, value: i32, uid: Uid, gid: Uid, pid: Tid) -> SysResult<()> {
    SEM_MANAGER.lock().setval(semid, semnum, value, uid, gid, pid)
}

pub fn setall(semid: usize, values: &[u16], uid: Uid, gid: Uid, pid: Tid) -> SysResult<()> {
    SEM_MANAGER.lock().setall(semid, values, uid, gid, pid)
}

pub fn remove_sem(semid: usize, uid: Uid) -> SysResult<()> {
    SEM_MANAGER.lock().remove(semid, uid)
}

pub fn begin_semop(semid: usize, ops: &[SemOp], uid: Uid, gid: Uid, pid: Tid) -> SysResult<SemopStatus> {
    SEM_MANAGER.lock().begin_semop(semid, ops, uid, gid, pid)
}

pub fn remove_current_waiter(semid: usize) {
    SEM_MANAGER.lock().remove_current_waiter(semid);
}
