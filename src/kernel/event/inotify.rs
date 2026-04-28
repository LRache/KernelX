use alloc::collections::{BTreeMap, VecDeque};
use alloc::string::String;
use alloc::sync::{Arc, Weak};
use alloc::vec::Vec;

use crate::fs::file::{FileFlags, FileOps};
use crate::fs::inode::{InotifyEvent, InotifyListener, InotifyRecord, Notifier};
use crate::fs::{Dentry, InodeOps, Mode};
use crate::kernel::errno::{Errno, SysResult};
use crate::kernel::event::{Event, FileEvent, WaitQueue};
use crate::kernel::mm::ubuf::UAddrSpaceBuffer;
use crate::kernel::scheduler::current;
use crate::kernel::scheduler::current::copy_to_user;
use crate::kernel::uapi::FileStat;
use crate::klib::SpinLock;

#[derive(Clone, PartialEq, Eq)]
struct QueuedInotifyRecord {
    wd: i32,
    mask: InotifyEvent,
    cookie: u32,
    name: String,
}

impl QueuedInotifyRecord {
    const HEADER_SIZE: usize = 16;
    const NAME_ALIGN: usize = 16;

    fn name_len(&self) -> usize {
        if self.name.is_empty() {
            0
        } else {
            (self.name.len() + 1 + Self::NAME_ALIGN - 1) & !(Self::NAME_ALIGN - 1)
        }
    }

    fn byte_size(&self) -> usize {
        Self::HEADER_SIZE + self.name_len()
    }

    fn write_to_slice(&self, buf: &mut [u8]) {
        buf[0..4].copy_from_slice(&self.wd.to_ne_bytes());
        buf[4..8].copy_from_slice(&self.mask.bits().to_ne_bytes());
        buf[8..12].copy_from_slice(&self.cookie.to_ne_bytes());
        buf[12..16].copy_from_slice(&(self.name_len() as u32).to_ne_bytes());

        let name_len = self.name_len();
        if name_len == 0 {
            return;
        }

        let name_buf = &mut buf[Self::HEADER_SIZE..Self::HEADER_SIZE + name_len];
        name_buf.fill(0);
        name_buf[..self.name.len()].copy_from_slice(self.name.as_bytes());
    }

    fn write_to_user(&self, uaddr: usize) -> SysResult<()> {
        copy_to_user::buffer(uaddr, &self.wd.to_ne_bytes())?;
        copy_to_user::buffer(uaddr + 4, &self.mask.bits().to_ne_bytes())?;
        copy_to_user::buffer(uaddr + 8, &self.cookie.to_ne_bytes())?;
        copy_to_user::buffer(uaddr + 12, &(self.name_len() as u32).to_ne_bytes())?;

        let name_len = self.name_len();
        if name_len == 0 {
            return Ok(());
        }

        let mut name = Vec::new();
        name.extend_from_slice(self.name.as_bytes());
        name.resize(name_len, 0);
        copy_to_user::buffer(uaddr + Self::HEADER_SIZE, &name)
    }
}

struct InotifyWatch {
    wd: i32,
    target_key: usize,
    mask: SpinLock<InotifyEvent>,
    inner: Weak<InotifyInner>,
}

impl InotifyWatch {
    fn new(wd: i32, target_key: usize, mask: InotifyEvent, inner: Weak<InotifyInner>) -> Self {
        Self {
            wd,
            target_key,
            mask: SpinLock::new(mask, "InotifyWatch::mask"),
            inner,
        }
    }

    fn update_mask(&self, mask: InotifyEvent) {
        let mut watch_mask = self.mask.lock();
        if mask.contains(InotifyEvent::MASK_ADD) {
            watch_mask.insert(mask);
        } else {
            *watch_mask = mask;
        }
    }
}

impl InotifyListener for InotifyWatch {
    fn notify(&self, record: &InotifyRecord) {
        let mask = *self.mask.lock();
        let mut base_event = record.mask;
        base_event.remove(InotifyEvent::ISDIR);
        let mut event = base_event & mask;
        if event.is_empty() {
            return;
        }
        if record.mask.contains(InotifyEvent::ISDIR) {
            event.insert(InotifyEvent::ISDIR);
        }

        let Some(inner) = self.inner.upgrade() else {
            return;
        };
        inner.push_event(QueuedInotifyRecord {
            wd: self.wd,
            mask: event,
            cookie: record.cookie,
            name: record.name.clone(),
        });
    }
}

struct InotifyInner {
    flags: SpinLock<FileFlags>,
    next_wd: SpinLock<i32>,
    watches: SpinLock<BTreeMap<i32, Arc<InotifyWatch>>>,
    targets: SpinLock<BTreeMap<usize, i32>>,
    events: SpinLock<VecDeque<QueuedInotifyRecord>>,
    read_waiter: SpinLock<WaitQueue<Event>>,
}

impl InotifyInner {
    fn new(blocked: bool) -> Arc<Self> {
        Arc::new(Self {
            flags: SpinLock::new(
                FileFlags {
                    readable: true,
                    writable: false,
                    blocked,
                    append: false,
                    direct: false,
                },
                "InotifyInner::flags",
            ),
            next_wd: SpinLock::new(1, "InotifyInner::next_wd"),
            watches: SpinLock::new(BTreeMap::new(), "InotifyInner::watches"),
            targets: SpinLock::new(BTreeMap::new(), "InotifyInner::targets"),
            events: SpinLock::new(VecDeque::new(), "InotifyInner::events"),
            read_waiter: SpinLock::new(WaitQueue::new(), "InotifyInner::read_waiter"),
        })
    }

    fn blocked(&self) -> bool {
        self.flags.lock().blocked
    }

    fn alloc_wd(&self) -> SysResult<i32> {
        let mut next_wd = self.next_wd.lock();
        let wd = *next_wd;
        *next_wd = next_wd.checked_add(1).filter(|wd| *wd > 0).ok_or(Errno::EMFILE)?;
        Ok(wd)
    }

    fn push_event(&self, record: QueuedInotifyRecord) {
        let was_empty = {
            let mut events = self.events.lock();
            let was_empty = events.is_empty();
            if events.back() == Some(&record) {
                return;
            }
            events.push_back(record);
            was_empty
        };

        if was_empty {
            self.read_waiter.lock().wake_all(|event| event);
        }
    }
}

pub struct InotifyFd {
    inner: Arc<InotifyInner>,
}

impl InotifyFd {
    pub fn new(blocked: bool) -> Self {
        Self {
            inner: InotifyInner::new(blocked),
        }
    }

    pub fn add_watch(&self, notifier: &Notifier, mask: InotifyEvent) -> SysResult<usize> {
        let target_key = notifier as *const Notifier as usize;

        let existing_wd = { self.inner.targets.lock().get(&target_key).copied() };
        if let Some(wd) = existing_wd {
            if mask.contains(InotifyEvent::MASK_CREATE) {
                return Err(Errno::EEXIST);
            }
            let watches = self.inner.watches.lock();
            let watch = watches.get(&wd).ok_or(Errno::EINVAL)?;
            watch.update_mask(mask);
            return Ok(wd as usize);
        }

        let wd = self.inner.alloc_wd()?;
        let watch = Arc::new(InotifyWatch::new(wd, target_key, mask, Arc::downgrade(&self.inner)));
        let listener: Arc<dyn InotifyListener> = watch.clone();
        notifier.subscribe(&listener);

        self.inner.watches.lock().insert(wd, watch);
        self.inner.targets.lock().insert(target_key, wd);

        Ok(wd as usize)
    }

    pub fn remove_watch(&self, wd: i32) -> SysResult<()> {
        let watch = self.inner.watches.lock().remove(&wd).ok_or(Errno::EINVAL)?;
        self.inner.targets.lock().remove(&watch.target_key);
        self.inner.push_event(QueuedInotifyRecord {
            wd,
            mask: InotifyEvent::IGNORED,
            cookie: 0,
            name: String::new(),
        });
        Ok(())
    }

    fn read_records(&self, len: usize) -> SysResult<Vec<QueuedInotifyRecord>> {
        if len < QueuedInotifyRecord::HEADER_SIZE {
            return Err(Errno::EINVAL);
        }

        loop {
            let mut events = self.inner.events.lock();
            if !events.is_empty() {
                if events.front().unwrap().byte_size() > len {
                    return Err(Errno::EINVAL);
                }

                let mut records = Vec::new();
                let mut bytes = 0;
                while let Some(record) = events.front() {
                    let record_size = record.byte_size();
                    if bytes + record_size > len {
                        break;
                    }
                    let record = events.pop_front().unwrap();
                    records.push(record);
                    bytes += record_size;
                }
                return Ok(records);
            }

            if !self.inner.blocked() {
                return Err(Errno::EAGAIN);
            }

            self.inner.read_waiter.lock().wait_current(Event::ReadReady);
            drop(events);

            current::schedule();
            match current::task().take_wakeup_event().unwrap() {
                Event::ReadReady => {}
                Event::Signal => {
                    self.inner.read_waiter.lock().remove(current::task());
                    return Err(Errno::EINTR);
                }
                event => unreachable!("unexpected event while waiting on inotify read: {:?}", event),
            }
        }
    }
}

impl FileOps for InotifyFd {
    fn read(&self, buf: &mut [u8]) -> SysResult<usize> {
        let records = self.read_records(buf.len())?;
        let mut bytes = 0;
        for record in records {
            let record_size = record.byte_size();
            record.write_to_slice(&mut buf[bytes..bytes + record_size]);
            bytes += record_size;
        }
        Ok(bytes)
    }

    fn read_to_user(&self, ubuf: &UAddrSpaceBuffer) -> SysResult<usize> {
        let records = self.read_records(ubuf.length())?;
        let mut bytes = 0;
        for record in records {
            record.write_to_user(ubuf.uaddr() + bytes)?;
            bytes += record.byte_size();
        }
        Ok(bytes)
    }

    fn write(&self, _buf: &[u8]) -> SysResult<usize> {
        Err(Errno::EINVAL)
    }

    fn flags(&self) -> FileFlags {
        *self.inner.flags.lock()
    }

    fn fstat(&self) -> SysResult<FileStat> {
        let mut kstat = FileStat::empty();
        kstat.st_ino = self as *const Self as *const () as u64;
        kstat.st_mode = (Mode::S_IFREG | Mode::S_IRUSR).bits();
        kstat.st_nlink = 1;
        Ok(kstat)
    }

    fn fsync(&self) -> SysResult<()> {
        Ok(())
    }

    fn get_inode(&self) -> Option<&Arc<dyn InodeOps>> {
        None
    }

    fn get_dentry(&self) -> Option<&Arc<Dentry>> {
        None
    }

    fn wait_event(&self, waker: usize, event: FileEvent) -> SysResult<Option<FileEvent>> {
        if !event.contains(FileEvent::READ_READY) {
            return Ok(None);
        }

        let events = self.inner.events.lock();
        if !events.is_empty() {
            return Ok(Some(FileEvent::READ_READY));
        }

        self.inner.read_waiter.lock().wait(
            current::task().clone(),
            Event::Poll {
                event: FileEvent::READ_READY,
                waker,
            },
        );
        drop(events);

        Ok(None)
    }

    fn wait_event_cancel(&self) {
        self.inner.read_waiter.lock().remove(current::task());
    }

    fn set_flags(&self, flags: FileFlags) {
        *self.inner.flags.lock() = FileFlags {
            readable: true,
            writable: false,
            blocked: flags.blocked,
            append: false,
            direct: false,
        };
    }

    fn type_name(&self) -> &'static str {
        "inotify"
    }
}
