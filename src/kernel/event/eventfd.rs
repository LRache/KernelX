use alloc::sync::Arc;
use core::mem::size_of;

use crate::fs::file::{FileFlags, FileOps};
use crate::fs::{Dentry, Inode, Mode};
use crate::kernel::errno::{Errno, SysResult};
use crate::kernel::event::{EpollNotifier, Event, FileEvent, WaitQueue};
use crate::kernel::mm::ubuf::UAddrSpaceBuffer;
use crate::kernel::scheduler::current;
use crate::kernel::scheduler::current::{copy_from_user, copy_to_user};
use crate::kernel::uapi::FileStat;
use crate::klib::SpinLock;

pub struct EventFd {
    counter: SpinLock<u64>,
    flags: SpinLock<FileFlags>,
    read_waiter: SpinLock<WaitQueue<Event>>,
    write_waiter: SpinLock<WaitQueue<Event>>,
    epoll_notifier: Arc<EpollNotifier>,
    semaphore: bool,
}

impl EventFd {
    const COUNTER_MAX: u64 = u64::MAX - 1;
    const IO_BYTES: usize = size_of::<u64>();

    pub fn new(initval: u64, blocked: bool, semaphore: bool) -> Self {
        debug_assert!(initval <= Self::COUNTER_MAX);
        Self {
            counter: SpinLock::new(initval, "EventFd::counter"),
            flags: SpinLock::new(
                FileFlags {
                    readable: true,
                    writable: true,
                    blocked,
                    append: false,
                    direct: false,
                },
                "EventFd::flags",
            ),
            read_waiter: SpinLock::new(WaitQueue::new("EventFd::read_waiter"), "EventFd::read_waiter"),
            write_waiter: SpinLock::new(WaitQueue::new("EventFd::write_waiter"), "EventFd::write_waiter"),
            epoll_notifier: Arc::new(EpollNotifier::new()),
            semaphore,
        }
    }

    fn validate_io_len(len: usize) -> SysResult<()> {
        if len >= Self::IO_BYTES {
            Ok(())
        } else {
            Err(Errno::EINVAL)
        }
    }

    fn blocked(&self) -> bool {
        self.flags.lock().blocked
    }

    fn is_write_ready(counter: u64) -> bool {
        counter < Self::COUNTER_MAX
    }

    fn can_write(counter: u64, value: u64) -> bool {
        value <= Self::COUNTER_MAX - counter
    }

    fn read_value(&self) -> SysResult<u64> {
        loop {
            let mut counter = self.counter.lock();
            if *counter != 0 {
                let (value, still_readable) = if self.semaphore {
                    *counter -= 1;
                    (1, *counter != 0)
                } else {
                    let value = *counter;
                    *counter = 0;
                    (value, false)
                };
                drop(counter);
                self.write_waiter.lock().wake_all(|event| event);
                self.epoll_notifier.notify(FileEvent::WRITE_READY);
                if still_readable {
                    self.epoll_notifier.notify(FileEvent::READ_READY);
                }
                return Ok(value);
            }

            if !self.blocked() {
                return Err(Errno::EAGAIN);
            }

            self.read_waiter.lock().wait_current(Event::ReadReady);
            drop(counter);

            current::schedule();
            match current::task().take_wakeup_event().unwrap() {
                Event::ReadReady => {}
                Event::Signal => {
                    self.read_waiter.lock().remove(current::task());
                    return Err(Errno::EINTR);
                }
                event => unreachable!("unexpected event while waiting on eventfd read: {:?}", event),
            }
        }
    }

    fn write_value(&self, value: u64) -> SysResult<()> {
        if value == u64::MAX {
            return Err(Errno::EINVAL);
        }

        loop {
            let mut counter = self.counter.lock();
            if Self::can_write(*counter, value) {
                let was_empty = *counter == 0;
                *counter += value;
                drop(counter);
                if value != 0 {
                    if was_empty {
                        self.read_waiter.lock().wake_all(|event| event);
                    }
                    self.epoll_notifier.notify(FileEvent::READ_READY);
                }
                return Ok(());
            }

            if !self.blocked() {
                return Err(Errno::EAGAIN);
            }

            self.write_waiter.lock().wait_current(Event::WriteReady);
            drop(counter);

            current::schedule();
            match current::task().take_wakeup_event().unwrap() {
                Event::WriteReady => {}
                Event::Signal => {
                    self.write_waiter.lock().remove(current::task());
                    return Err(Errno::EINTR);
                }
                event => unreachable!("unexpected event while waiting on eventfd write: {:?}", event),
            }
        }
    }
}

impl FileOps for EventFd {
    fn read(&self, buf: &mut [u8]) -> SysResult<usize> {
        Self::validate_io_len(buf.len())?;
        let value = self.read_value()?;
        buf[..Self::IO_BYTES].copy_from_slice(&value.to_ne_bytes());
        Ok(Self::IO_BYTES)
    }

    fn read_to_user(&self, ubuf: &UAddrSpaceBuffer) -> SysResult<usize> {
        Self::validate_io_len(ubuf.length())?;
        let value = self.read_value()?;
        copy_to_user::buffer(ubuf.uaddr(), &value.to_ne_bytes())?;
        Ok(Self::IO_BYTES)
    }

    fn write(&self, buf: &[u8]) -> SysResult<usize> {
        Self::validate_io_len(buf.len())?;
        let value = u64::from_ne_bytes(buf[..Self::IO_BYTES].try_into().unwrap());
        self.write_value(value)?;
        Ok(Self::IO_BYTES)
    }

    fn write_from_user(&self, ubuf: &UAddrSpaceBuffer) -> SysResult<usize> {
        Self::validate_io_len(ubuf.length())?;
        let mut bytes = [0u8; Self::IO_BYTES];
        copy_from_user::buffer(ubuf.uaddr(), &mut bytes)?;
        self.write_value(u64::from_ne_bytes(bytes))?;
        Ok(Self::IO_BYTES)
    }

    fn flags(&self) -> FileFlags {
        *self.flags.lock()
    }

    fn fstat(&self) -> SysResult<FileStat> {
        let mut kstat = FileStat::empty();
        kstat.st_ino = self as *const Self as *const () as u64;
        kstat.st_mode = (Mode::S_IFREG | Mode::S_IRUSR | Mode::S_IWUSR).bits();
        kstat.st_nlink = 1;
        Ok(kstat)
    }

    fn fsync(&self) -> SysResult<()> {
        Ok(())
    }

    fn get_inode(&self) -> Option<&Arc<Inode>> {
        None
    }

    fn get_dentry(&self) -> Option<&Arc<Dentry>> {
        None
    }

    fn poll_event(&self, event: FileEvent) -> SysResult<Option<FileEvent>> {
        let want_read = event.contains(FileEvent::READ_READY);
        let want_write = event.contains(FileEvent::WRITE_READY);
        if !want_read && !want_write {
            return Ok(None);
        }

        let counter = self.counter.lock();
        let mut ready = FileEvent::empty();

        if want_read && *counter != 0 {
            ready |= FileEvent::READ_READY;
        }
        if want_write && Self::is_write_ready(*counter) {
            ready |= FileEvent::WRITE_READY;
        }

        if !ready.is_empty() {
            return Ok(Some(ready));
        }

        Ok(None)
    }

    fn wait_event(&self, waker: usize, event: FileEvent) -> SysResult<Option<FileEvent>> {
        let want_read = event.contains(FileEvent::READ_READY);
        let want_write = event.contains(FileEvent::WRITE_READY);
        if !want_read && !want_write {
            return Ok(None);
        }

        let counter = self.counter.lock();

        let mut ready = FileEvent::empty();
        if want_read && *counter != 0 {
            ready |= FileEvent::READ_READY;
        }
        if want_write && Self::is_write_ready(*counter) {
            ready |= FileEvent::WRITE_READY;
        }

        if !ready.is_empty() {
            return Ok(Some(ready));
        }

        if want_read {
            self.read_waiter.lock().wait(
                current::task().clone(),
                Event::Poll {
                    event: FileEvent::READ_READY,
                    waker,
                },
            );
        }
        if want_write {
            self.write_waiter.lock().wait(
                current::task().clone(),
                Event::Poll {
                    event: FileEvent::WRITE_READY,
                    waker,
                },
            );
        }

        Ok(None)
    }

    fn wait_event_cancel(&self) {
        self.read_waiter.lock().remove(current::task());
        self.write_waiter.lock().remove(current::task());
    }

    fn epoll_notifier(&self) -> Option<Arc<EpollNotifier>> {
        Some(self.epoll_notifier.clone())
    }

    fn set_flags(&self, flags: FileFlags) {
        *self.flags.lock() = FileFlags {
            readable: true,
            writable: true,
            blocked: flags.blocked,
            append: false,
            direct: false,
        };
    }

    fn type_name(&self) -> &'static str {
        "eventfd"
    }
}
