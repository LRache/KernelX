use alloc::collections::{BTreeMap, VecDeque};
use alloc::vec::Vec;

use crate::kernel::errno::{Errno, SysResult};
use crate::kernel::event::{Event, WaitQueue};
use crate::kernel::ipc::{IPC_PRIVATE, IpcGetFlag, IpcMode};
use crate::kernel::scheduler::Tid;
use crate::kernel::uapi::Uid;
use crate::klib::SpinLock;

pub mod limit {
    pub const MAX_QUEUES: usize = 32000;
    pub const MAX_MESSAGE_SIZE: usize = 8192;
    pub const DEFAULT_QUEUE_BYTES: usize = 16384;
    pub const MAX_QUEUE_BYTES: usize = DEFAULT_QUEUE_BYTES * 16;

    pub fn usize_to_i32(value: usize) -> i32 {
        value.min(i32::MAX as usize) as i32
    }
}

#[derive(Clone, Copy, Debug)]
pub struct MsgidDs {
    pub key: usize,
    pub uid: Uid,
    pub gid: Uid,
    pub cuid: Uid,
    pub cgid: Uid,
    pub mode: u32,
    pub stime: usize,
    pub rtime: usize,
    pub ctime: usize,
    pub cbytes: usize,
    pub qnum: usize,
    pub qbytes: usize,
    pub lspid: Tid,
    pub lrpid: Tid,
}

#[derive(Clone, Copy, Debug)]
pub struct MsgStat {
    pub ds: MsgidDs,
}

#[derive(Clone, Copy, Debug)]
pub struct MsgInfo {
    pub msgpool: i32,
    pub msgmap: i32,
    pub msgmax: i32,
    pub msgmnb: i32,
    pub msgmni: i32,
    pub msgssz: i32,
    pub msgtql: i32,
    pub msgseg: u16,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MsgStatus {
    Done,
    WouldBlock,
}

#[derive(Clone, Copy, Debug)]
pub enum MsgSelector {
    Any,
    Exact(isize),
    Except(isize),
    LessOrEqual(isize),
}

pub struct MsgReceive {
    pub mtype: isize,
    pub data: Vec<u8>,
}

struct MsgMessage {
    mtype: isize,
    data: Vec<u8>,
}

#[derive(Clone, Copy, Debug)]
enum MsgWait {
    Send,
    Receive,
}

struct MsgIdentifier {
    ds: MsgidDs,
    messages: VecDeque<MsgMessage>,
    waiters: WaitQueue<MsgWait>,
    deleted: bool,
}

pub struct MsgManager {
    queues: BTreeMap<usize, MsgIdentifier>,
    next_msgid: usize,
}

fn has_msg_perm(msg: &MsgIdentifier, uid: Uid, gid: Uid, write: bool) -> bool {
    if uid == 0 {
        return true;
    }

    let required = if write { IpcMode::WRITE } else { IpcMode::READ };
    let shift = if uid == msg.ds.uid {
        6
    } else if gid == msg.ds.gid {
        3
    } else {
        0
    };
    let allowed = IpcMode::from_bits_truncate((msg.ds.mode >> shift) & IpcMode::ALL.bits());

    allowed.contains(required)
}

fn has_msg_get_perm(msg: &MsgIdentifier, mode: u32, uid: Uid, gid: Uid) -> bool {
    if uid == 0 {
        return true;
    }

    let access_mask = IpcMode::ALL.bits() >> 6;
    let requested = ((mode >> 6) | (mode >> 3) | mode) & access_mask;
    let shift = if uid == msg.ds.uid {
        6
    } else if gid == msg.ds.gid {
        3
    } else {
        0
    };
    let granted = (msg.ds.mode >> shift) & access_mask;

    requested & !granted == 0
}

fn matches_selector(message: &MsgMessage, selector: MsgSelector) -> bool {
    match selector {
        MsgSelector::Any => true,
        MsgSelector::Exact(mtype) => message.mtype == mtype,
        MsgSelector::Except(mtype) => message.mtype != mtype,
        MsgSelector::LessOrEqual(max_type) => message.mtype <= max_type,
    }
}

fn find_message_index(messages: &VecDeque<MsgMessage>, selector: MsgSelector) -> Option<usize> {
    match selector {
        MsgSelector::LessOrEqual(max_type) => {
            let mut best = None;
            for (index, message) in messages.iter().enumerate() {
                if message.mtype > max_type {
                    continue;
                }
                if best.map(|(_, best_type)| message.mtype < best_type).unwrap_or(true) {
                    best = Some((index, message.mtype));
                }
            }
            best.map(|(index, _)| index)
        }
        _ => messages
            .iter()
            .enumerate()
            .find(|(_, message)| matches_selector(message, selector))
            .map(|(index, _)| index),
    }
}

impl MsgManager {
    const fn new() -> Self {
        Self {
            queues: BTreeMap::new(),
            next_msgid: 1,
        }
    }

    fn get_or_create(&mut self, key: usize, flags: IpcGetFlag, mode: u32, uid: Uid, gid: Uid) -> SysResult<usize> {
        if key != IPC_PRIVATE {
            let existing = self
                .queues
                .iter()
                .find(|(_, msg)| !msg.deleted && msg.ds.key == key)
                .map(|(id, _)| *id);

            if let Some(id) = existing {
                if flags.contains(IpcGetFlag::IPC_CREAT | IpcGetFlag::IPC_EXCL) {
                    return Err(Errno::EEXIST);
                }
                let msg = self.queues.get(&id).unwrap();
                if !has_msg_get_perm(msg, mode, uid, gid) {
                    return Err(Errno::EACCES);
                }
                return Ok(id);
            }
        }

        if key != IPC_PRIVATE && !flags.contains(IpcGetFlag::IPC_CREAT) {
            return Err(Errno::ENOENT);
        }
        if self.queues.len() >= limit::MAX_QUEUES {
            return Err(Errno::ENOSPC);
        }

        let id = self.next_msgid;
        self.next_msgid = self.next_msgid.checked_add(1).ok_or(Errno::ENOSPC)?;

        self.queues.insert(
            id,
            MsgIdentifier {
                ds: MsgidDs {
                    key,
                    uid,
                    gid,
                    cuid: uid,
                    cgid: gid,
                    mode,
                    stime: 0,
                    rtime: 0,
                    ctime: 0,
                    cbytes: 0,
                    qnum: 0,
                    qbytes: limit::DEFAULT_QUEUE_BYTES,
                    lspid: 0,
                    lrpid: 0,
                },
                messages: VecDeque::new(),
                waiters: WaitQueue::new(),
                deleted: false,
            },
        );

        Ok(id)
    }

    fn stat(&self, msgid: usize, uid: Uid, gid: Uid) -> SysResult<MsgStat> {
        let msg = self.queues.get(&msgid).ok_or(Errno::EINVAL)?;
        if msg.deleted {
            return Err(Errno::EINVAL);
        }
        if !has_msg_perm(msg, uid, gid, false) {
            return Err(Errno::EACCES);
        }
        Ok(MsgStat { ds: msg.ds })
    }

    fn stat_index(&self, index: usize, uid: Uid, gid: Uid) -> SysResult<(usize, MsgStat)> {
        let msg = self.queues.get(&index).ok_or(Errno::EINVAL)?;
        if msg.deleted {
            return Err(Errno::EINVAL);
        }
        if !has_msg_perm(msg, uid, gid, false) {
            return Err(Errno::EACCES);
        }
        Ok((index, MsgStat { ds: msg.ds }))
    }

    fn info(&self, usage: bool) -> (usize, MsgInfo) {
        let mut highest_index = 0;
        let mut queue_count = 0;
        let mut message_count = 0;
        let mut byte_count = 0;

        for (id, msg) in &self.queues {
            if msg.deleted {
                continue;
            }
            highest_index = highest_index.max(*id);
            queue_count += 1;
            message_count += msg.ds.qnum;
            byte_count += msg.ds.cbytes;
        }

        let (msgpool, msgmap, msgtql) = if usage {
            (
                limit::usize_to_i32(queue_count),
                limit::usize_to_i32(byte_count),
                limit::usize_to_i32(message_count),
            )
        } else {
            (
                limit::usize_to_i32(limit::MAX_QUEUES),
                limit::usize_to_i32(limit::MAX_QUEUE_BYTES),
                limit::usize_to_i32(limit::MAX_QUEUES * limit::DEFAULT_QUEUE_BYTES),
            )
        };

        (
            highest_index,
            MsgInfo {
                msgpool,
                msgmap,
                msgmax: limit::usize_to_i32(limit::MAX_MESSAGE_SIZE),
                msgmnb: limit::usize_to_i32(limit::DEFAULT_QUEUE_BYTES),
                msgmni: limit::usize_to_i32(limit::MAX_QUEUES),
                msgssz: 16,
                msgtql,
                msgseg: u16::MAX,
            },
        )
    }

    fn set_perm(
        &mut self,
        msgid: usize,
        uid: Uid,
        gid: Uid,
        mode: u32,
        qbytes: usize,
        caller_uid: Uid,
    ) -> SysResult<()> {
        let msg = self.queues.get_mut(&msgid).ok_or(Errno::EINVAL)?;
        if msg.deleted {
            return Err(Errno::EIDRM);
        }
        if caller_uid != 0 && caller_uid != msg.ds.uid && caller_uid != msg.ds.cuid {
            return Err(Errno::EPERM);
        }
        if qbytes == 0 || qbytes > limit::MAX_QUEUE_BYTES {
            return Err(Errno::EINVAL);
        }
        if qbytes > limit::DEFAULT_QUEUE_BYTES && caller_uid != 0 {
            return Err(Errno::EPERM);
        }

        msg.ds.uid = uid;
        msg.ds.gid = gid;
        msg.ds.mode = mode & IpcMode::ALL.bits();
        msg.ds.qbytes = qbytes;
        msg.ds.ctime = 0;
        msg.waiters.wake_all(|_| Event::Msg);
        Ok(())
    }

    fn remove(&mut self, msgid: usize, uid: Uid) -> SysResult<()> {
        let msg = self.queues.get_mut(&msgid).ok_or(Errno::EINVAL)?;
        if uid != 0 && uid != msg.ds.uid && uid != msg.ds.cuid {
            return Err(Errno::EPERM);
        }
        msg.deleted = true;
        msg.waiters.wake_all(|_| Event::Msg);
        Ok(())
    }

    fn begin_send(
        &mut self,
        msgid: usize,
        mtype: isize,
        data: &[u8],
        nowait: bool,
        uid: Uid,
        gid: Uid,
        pid: Tid,
        time: usize,
    ) -> SysResult<MsgStatus> {
        if mtype <= 0 || data.len() > limit::MAX_MESSAGE_SIZE {
            return Err(Errno::EINVAL);
        }

        let msg = self.queues.get_mut(&msgid).ok_or(Errno::EINVAL)?;
        if msg.deleted {
            return Err(Errno::EIDRM);
        }
        if !has_msg_perm(msg, uid, gid, true) {
            return Err(Errno::EACCES);
        }

        let would_exceed_bytes = msg.ds.cbytes.checked_add(data.len()).ok_or(Errno::EINVAL)? > msg.ds.qbytes;
        let would_exceed_messages = msg.ds.qnum >= msg.ds.qbytes;
        if would_exceed_bytes || would_exceed_messages {
            if nowait {
                return Err(Errno::EAGAIN);
            }
            msg.waiters.wait_current(MsgWait::Send);
            return Ok(MsgStatus::WouldBlock);
        }

        msg.messages.push_back(MsgMessage {
            mtype,
            data: data.to_vec(),
        });
        msg.ds.cbytes += data.len();
        msg.ds.qnum += 1;
        msg.ds.lspid = pid;
        msg.ds.stime = time;
        msg.waiters.wake_all(|_| Event::Msg);

        Ok(MsgStatus::Done)
    }

    fn begin_receive(
        &mut self,
        msgid: usize,
        selector: MsgSelector,
        msgsz: usize,
        noerror: bool,
        nowait: bool,
        uid: Uid,
        gid: Uid,
        pid: Tid,
        time: usize,
    ) -> SysResult<Result<MsgReceive, MsgStatus>> {
        let msg = self.queues.get_mut(&msgid).ok_or(Errno::EINVAL)?;
        if msg.deleted {
            return Err(Errno::EIDRM);
        }
        if !has_msg_perm(msg, uid, gid, false) {
            return Err(Errno::EACCES);
        }

        let Some(index) = find_message_index(&msg.messages, selector) else {
            if nowait {
                return Err(Errno::ENOMSG);
            }
            msg.waiters.wait_current(MsgWait::Receive);
            return Ok(Err(MsgStatus::WouldBlock));
        };

        let message = msg.messages.get(index).unwrap();
        if message.data.len() > msgsz && !noerror {
            return Err(Errno::E2BIG);
        }

        let message = msg.messages.remove(index).unwrap();
        let copy_len = core::cmp::min(message.data.len(), msgsz);
        msg.ds.cbytes -= message.data.len();
        msg.ds.qnum -= 1;
        msg.ds.lrpid = pid;
        msg.ds.rtime = time;
        msg.waiters.wake_all(|_| Event::Msg);

        Ok(Ok(MsgReceive {
            mtype: message.mtype,
            data: message.data[..copy_len].to_vec(),
        }))
    }

    fn copy_receive(
        &self,
        msgid: usize,
        index: usize,
        msgsz: usize,
        noerror: bool,
        uid: Uid,
        gid: Uid,
    ) -> SysResult<MsgReceive> {
        let msg = self.queues.get(&msgid).ok_or(Errno::EINVAL)?;
        if msg.deleted {
            return Err(Errno::EINVAL);
        }
        if !has_msg_perm(msg, uid, gid, false) {
            return Err(Errno::EACCES);
        }

        let message = msg.messages.get(index).ok_or(Errno::ENOMSG)?;
        if message.data.len() > msgsz && !noerror {
            return Err(Errno::E2BIG);
        }

        let copy_len = core::cmp::min(message.data.len(), msgsz);
        Ok(MsgReceive {
            mtype: message.mtype,
            data: message.data[..copy_len].to_vec(),
        })
    }

    fn remove_current_waiter(&mut self, msgid: usize) {
        if let Some(msg) = self.queues.get_mut(&msgid) {
            msg.waiters.remove_current();
        }
    }
}

static MSG_MANAGER: SpinLock<MsgManager> = SpinLock::new(MsgManager::new(), "static::MSG_MANAGER");

pub fn get_or_create_msg(key: usize, flags: IpcGetFlag, mode: u32, uid: Uid, gid: Uid) -> SysResult<usize> {
    MSG_MANAGER.lock().get_or_create(key, flags, mode, uid, gid)
}

pub fn stat_msg(msgid: usize, uid: Uid, gid: Uid) -> SysResult<MsgStat> {
    MSG_MANAGER.lock().stat(msgid, uid, gid)
}

pub fn stat_index_msg(index: usize, uid: Uid, gid: Uid) -> SysResult<(usize, MsgStat)> {
    MSG_MANAGER.lock().stat_index(index, uid, gid)
}

pub fn info_msg(usage: bool) -> (usize, MsgInfo) {
    MSG_MANAGER.lock().info(usage)
}

pub fn set_perm_msg(msgid: usize, uid: Uid, gid: Uid, mode: u32, qbytes: usize, caller_uid: Uid) -> SysResult<()> {
    MSG_MANAGER.lock().set_perm(msgid, uid, gid, mode, qbytes, caller_uid)
}

pub fn remove_msg(msgid: usize, uid: Uid) -> SysResult<()> {
    MSG_MANAGER.lock().remove(msgid, uid)
}

pub fn begin_msgsnd(
    msgid: usize,
    mtype: isize,
    data: &[u8],
    nowait: bool,
    uid: Uid,
    gid: Uid,
    pid: Tid,
    time: usize,
) -> SysResult<MsgStatus> {
    MSG_MANAGER
        .lock()
        .begin_send(msgid, mtype, data, nowait, uid, gid, pid, time)
}

pub fn begin_msgrcv(
    msgid: usize,
    selector: MsgSelector,
    msgsz: usize,
    noerror: bool,
    nowait: bool,
    uid: Uid,
    gid: Uid,
    pid: Tid,
    time: usize,
) -> SysResult<Result<MsgReceive, MsgStatus>> {
    MSG_MANAGER
        .lock()
        .begin_receive(msgid, selector, msgsz, noerror, nowait, uid, gid, pid, time)
}

pub fn copy_msgrcv(
    msgid: usize,
    index: usize,
    msgsz: usize,
    noerror: bool,
    uid: Uid,
    gid: Uid,
) -> SysResult<MsgReceive> {
    MSG_MANAGER.lock().copy_receive(msgid, index, msgsz, noerror, uid, gid)
}

pub fn remove_current_waiter(msgid: usize) {
    MSG_MANAGER.lock().remove_current_waiter(msgid);
}
