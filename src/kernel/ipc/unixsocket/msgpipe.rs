use alloc::collections::VecDeque;
use alloc::vec::Vec;

use crate::kernel::errno::{Errno, SysResult};
use crate::kernel::event::{Event, FileEvent, PollEventSet, WaitQueue};
use crate::kernel::mm::ubuf::UAddrSpaceBuffer;
use crate::kernel::scheduler::current;
use crate::klib::{SleepLock, SpinLock};

struct MessageQueue {
    messages: VecDeque<Vec<u8>>,
    total_bytes: usize,
    capacity: usize,
}

impl MessageQueue {
    fn new(capacity: usize) -> Self {
        Self {
            messages: VecDeque::new(),
            total_bytes: 0,
            capacity,
        }
    }

    fn is_empty(&self) -> bool {
        self.messages.is_empty()
    }

    fn available_space(&self) -> usize {
        self.capacity.saturating_sub(self.total_bytes)
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
    writer_count: SpinLock<u32>,
    reader_count: SpinLock<u32>,
}

impl MessagePipeInner {
    pub fn new(capacity: usize) -> Self {
        Self {
            queue: SleepLock::new(MessageQueue::new(capacity), "MessagePipeInner::queue"),
            read_waiter: SpinLock::new(WaitQueue::new(), "MessagePipeInner::read_waiter"),
            write_waiter: SpinLock::new(WaitQueue::new(), "MessagePipeInner::write_waiter"),
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
                Event::Signal => return Err(Errno::EINTR),
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
                Event::Signal => return Err(Errno::EINTR),
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
            if queue.available_space() >= buf.len() {
                let msg = Vec::from(buf);
                queue.push(msg);
                drop(queue);
                self.read_waiter.lock().wake_all(|e| e);
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
                Event::Signal => return Err(Errno::EINTR),
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
            if queue.available_space() >= msg.len() {
                let len = msg.len();
                queue.push(msg);
                drop(queue);
                self.read_waiter.lock().wake_all(|e| e);
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
                Event::Signal => return Err(Errno::EINTR),
                _ => unreachable!(),
            }
        }
    }

    pub fn wait_event(&self, waker: usize, event: PollEventSet, is_writer: bool) -> SysResult<Option<FileEvent>> {
        if event.contains(PollEventSet::POLLIN) && is_writer {
            return Ok(None);
        }
        if event.contains(PollEventSet::POLLOUT) && !is_writer {
            return Ok(None);
        }

        let queue = self.queue.lock();

        if event.contains(PollEventSet::POLLIN) {
            if *self.writer_count.lock() == 0 {
                return Ok(Some(FileEvent::HangUp));
            }
            if !queue.is_empty() {
                return Ok(Some(FileEvent::ReadReady));
            }
            self.read_waiter.lock().wait(
                current::task().clone(),
                Event::Poll {
                    event: FileEvent::ReadReady,
                    waker,
                },
            );
        }

        if event.contains(PollEventSet::POLLOUT) {
            if *self.reader_count.lock() == 0 {
                return Ok(Some(FileEvent::HangUp));
            }
            if queue.available_space() > 0 {
                return Ok(Some(FileEvent::WriteReady));
            }
            self.write_waiter.lock().wait(
                current::task().clone(),
                Event::Poll {
                    event: FileEvent::WriteReady,
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

    pub fn increment_reader_count(&self) {
        *self.reader_count.lock() += 1;
    }

    pub fn decrement_reader_count(&self) {
        let mut count = self.reader_count.lock();
        debug_assert!(*count > 0);
        *count -= 1;
        if *count == 0 {
            self.write_waiter.lock().wake_all(|e| e);
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
            self.read_waiter.lock().wake_all(|e| match e {
                Event::Poll {
                    event: FileEvent::ReadReady,
                    waker,
                } => Event::Poll {
                    event: FileEvent::HangUp,
                    waker,
                },
                _ => e,
            });
        }
    }
}
