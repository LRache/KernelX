use alloc::collections::VecDeque;
use alloc::sync::Arc;
use alloc::vec::Vec;

use crate::kernel::errno::{Errno, SysResult};
use crate::kernel::event::{EpollNotifier, Event, FileEvent, WaitQueue};
use crate::kernel::mm::ubuf::UAddrSpaceBuffer;
use crate::kernel::scheduler::current;
use crate::klib::{SleepLock, SpinLock};

struct MessageQueue {
    messages: VecDeque<Vec<u8>>,
    total_bytes: usize,
    capacity: usize,
    max_messages: usize,
}

impl MessageQueue {
    fn new(capacity: usize) -> Self {
        Self {
            messages: VecDeque::new(),
            total_bytes: 0,
            capacity,
            max_messages: capacity / 2,
        }
    }

    fn is_empty(&self) -> bool {
        self.messages.is_empty()
    }

    fn available_space(&self) -> usize {
        self.capacity.saturating_sub(self.total_bytes)
    }

    fn available_messages(&self) -> usize {
        self.max_messages.saturating_sub(self.messages.len())
    }

    fn can_push(&self, msg_len: usize) -> bool {
        self.available_space() >= msg_len && self.available_messages() > 0
    }

    fn push(&mut self, msg: Vec<u8>) {
        self.total_bytes += msg.len();
        self.messages.push_back(msg);
    }

    fn pop(&mut self) -> Option<Vec<u8>> {
        if let Some(msg) = self.messages.pop_front() {
            self.total_bytes -= msg.len();
            Some(msg)
        } else {
            None
        }
    }
}

/// Message-oriented pipe for SOCK_DGRAM and SOCK_SEQPACKET.
///
/// Each write produces one message; each read consumes one message.
/// If the read buffer is smaller than the message, the rest is discarded.
pub struct MessagePipeInner {
    queue: SleepLock<MessageQueue>,
    read_waiter: SpinLock<WaitQueue<Event>>,
    write_waiter: SpinLock<WaitQueue<Event>>,
    read_notifier: Arc<EpollNotifier>,
    write_notifier: Arc<EpollNotifier>,
    writer_count: SpinLock<u32>,
    reader_count: SpinLock<u32>,
}

impl MessagePipeInner {
    pub fn new(capacity: usize) -> Self {
        Self {
            queue: SleepLock::new(MessageQueue::new(capacity), "MessagePipeInner::queue"),
            read_waiter: SpinLock::new(WaitQueue::new(), "MessagePipeInner::read_waiter"),
            write_waiter: SpinLock::new(WaitQueue::new(), "MessagePipeInner::write_waiter"),
            read_notifier: Arc::new(EpollNotifier::new()),
            write_notifier: Arc::new(EpollNotifier::new()),
            writer_count: SpinLock::new(0, "MessagePipeInner::writer_count"),
            reader_count: SpinLock::new(0, "MessagePipeInner::reader_count"),
        }
    }

    pub fn read(&self, buf: &mut [u8], blocked: bool) -> SysResult<usize> {
        if buf.is_empty() {
            return Ok(0);
        }

        loop {
            let mut queue = self.queue.lock();
            if let Some(msg) = queue.pop() {
                let copy_len = core::cmp::min(buf.len(), msg.len());
                buf[..copy_len].copy_from_slice(&msg[..copy_len]);
                drop(queue);
                self.write_waiter.lock().wake_all(|e| e);
                self.write_notifier.notify(FileEvent::WRITE_READY);
                return Ok(copy_len);
            }

            if *self.writer_count.lock() == 0 {
                return Ok(0); // EOF
            }

            drop(queue);

            if !blocked {
                return Err(Errno::EAGAIN);
            }

            self.read_waiter.lock().wait_current(Event::ReadReady);
            current::schedule();
            match current::task().take_wakeup_event().unwrap() {
                Event::ReadReady => {}
                Event::Signal => {
                    self.wait_event_cancel();
                    return Err(Errno::EINTR);
                }
                _ => unreachable!(),
            }
        }
    }

    pub fn read_to_user(&self, ubuf: &UAddrSpaceBuffer, blocked: bool) -> SysResult<usize> {
        if ubuf.length() == 0 {
            return Ok(0);
        }

        loop {
            let mut queue = self.queue.lock();
            if let Some(msg) = queue.pop() {
                let copy_len = core::cmp::min(ubuf.length(), msg.len());
                ubuf.write(0, &msg[..copy_len])?;
                drop(queue);
                self.write_waiter.lock().wake_all(|e| e);
                self.write_notifier.notify(FileEvent::WRITE_READY);
                return Ok(copy_len);
            }

            if *self.writer_count.lock() == 0 {
                return Ok(0); // EOF
            }

            drop(queue);

            if !blocked {
                return Err(Errno::EAGAIN);
            }

            self.read_waiter.lock().wait_current(Event::ReadReady);
            current::schedule();
            match current::task().take_wakeup_event().unwrap() {
                Event::ReadReady => {}
                Event::Signal => {
                    self.wait_event_cancel();
                    return Err(Errno::EINTR);
                }
                _ => unreachable!(),
            }
        }
    }

    pub fn write(&self, buf: &[u8], blocked: bool) -> SysResult<usize> {
        if *self.reader_count.lock() == 0 {
            return Err(Errno::EPIPE);
        }

        loop {
            let mut queue = self.queue.lock();
            if queue.can_push(buf.len()) {
                let msg = Vec::from(buf);
                queue.push(msg);
                drop(queue);
                self.read_waiter.lock().wake_all(|e| e);
                self.read_notifier.notify(FileEvent::READ_READY);
                return Ok(buf.len());
            }
            drop(queue);

            if !blocked {
                return Err(Errno::EAGAIN);
            }

            if *self.reader_count.lock() == 0 {
                return Err(Errno::EPIPE);
            }

            self.write_waiter.lock().wait_current(Event::WriteReady);
            current::schedule();
            match current::task().take_wakeup_event().unwrap() {
                Event::WriteReady => {}
                Event::Signal => {
                    self.wait_event_cancel();
                    return Err(Errno::EINTR);
                }
                _ => unreachable!(),
            }
        }
    }

    pub fn write_from_user(&self, ubuf: &UAddrSpaceBuffer, blocked: bool) -> SysResult<usize> {
        if *self.reader_count.lock() == 0 {
            return Err(Errno::EPIPE);
        }

        // Collect user data into a kernel buffer first
        let mut msg = Vec::with_capacity(ubuf.length());
        for kbuf in ubuf.iter() {
            let kbuf = kbuf?;
            msg.extend_from_slice(kbuf);
        }

        loop {
            let mut queue = self.queue.lock();
            if queue.can_push(msg.len()) {
                let len = msg.len();
                queue.push(msg);
                drop(queue);
                self.read_waiter.lock().wake_all(|e| e);
                self.read_notifier.notify(FileEvent::READ_READY);
                return Ok(len);
            }
            drop(queue);

            if !blocked {
                return Err(Errno::EAGAIN);
            }

            if *self.reader_count.lock() == 0 {
                return Err(Errno::EPIPE);
            }

            self.write_waiter.lock().wait_current(Event::WriteReady);
            current::schedule();
            match current::task().take_wakeup_event().unwrap() {
                Event::WriteReady => {}
                Event::Signal => {
                    self.wait_event_cancel();
                    return Err(Errno::EINTR);
                }
                _ => unreachable!(),
            }
        }
    }

    pub fn poll_event(&self, event: FileEvent, is_writer: bool) -> SysResult<Option<FileEvent>> {
        let want_read = event.contains(FileEvent::READ_READY) && !is_writer;
        let want_write = event.contains(FileEvent::WRITE_READY) && is_writer;
        if !want_read && !want_write {
            return Ok(None);
        }

        let queue = self.queue.lock();
        let mut ready = FileEvent::empty();

        if want_read {
            if *self.writer_count.lock() == 0 {
                ready |= FileEvent::HANG_UP;
                if !queue.is_empty() {
                    ready |= FileEvent::READ_READY;
                }
            } else if !queue.is_empty() {
                ready |= FileEvent::READ_READY;
            }
        }

        if want_write {
            if *self.reader_count.lock() == 0 {
                ready |= FileEvent::HANG_UP;
            } else if queue.available_space() > 0 && queue.available_messages() > 0 {
                ready |= FileEvent::WRITE_READY;
            }
        }

        if ready.is_empty() { Ok(None) } else { Ok(Some(ready)) }
    }

    pub fn wait_event(&self, waker: usize, event: FileEvent, is_writer: bool) -> SysResult<Option<FileEvent>> {
        let want_read = event.contains(FileEvent::READ_READY) && !is_writer;
        let want_write = event.contains(FileEvent::WRITE_READY) && is_writer;
        if !want_read && !want_write {
            return Ok(None);
        }

        let queue = self.queue.lock();
        let mut ready = FileEvent::empty();

        if want_read {
            if *self.writer_count.lock() == 0 {
                ready |= FileEvent::HANG_UP;
                if !queue.is_empty() {
                    ready |= FileEvent::READ_READY;
                }
            } else if !queue.is_empty() {
                ready |= FileEvent::READ_READY;
            }
        }

        if want_write {
            if *self.reader_count.lock() == 0 {
                ready |= FileEvent::HANG_UP;
            } else if queue.available_space() > 0 && queue.available_messages() > 0 {
                ready |= FileEvent::WRITE_READY;
            }
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

    pub fn wait_event_cancel(&self) {
        self.read_waiter.lock().remove(current::task());
        self.write_waiter.lock().remove(current::task());
    }

    pub fn epoll_notifier(&self, is_writer: bool) -> Arc<EpollNotifier> {
        if is_writer {
            self.write_notifier.clone()
        } else {
            self.read_notifier.clone()
        }
    }

    pub fn increment_reader_count(&self) {
        *self.reader_count.lock() += 1;
    }

    pub fn decrement_reader_count(&self) {
        let mut count = self.reader_count.lock();
        debug_assert!(*count > 0);
        *count -= 1;
        if *count == 0 {
            self.write_waiter.lock().wake_all(|e| e);
            self.write_notifier.notify(FileEvent::HANG_UP);
        }
    }

    pub fn increment_writer_count(&self) {
        *self.writer_count.lock() += 1;
    }

    pub fn decrement_writer_count(&self) {
        let mut count = self.writer_count.lock();
        debug_assert!(*count > 0);
        *count -= 1;
        if *count == 0 {
            let wake_event = if !self.queue.lock().is_empty() {
                FileEvent::READ_READY | FileEvent::HANG_UP
            } else {
                FileEvent::HANG_UP
            };
            self.read_waiter.lock().wake_all(|e| match e {
                Event::Poll { event, waker } if event.intersects(FileEvent::READ_READY) => Event::Poll {
                    event: wake_event,
                    waker,
                },
                _ => e,
            });
            self.read_notifier.notify(wake_event);
        }
    }
}
