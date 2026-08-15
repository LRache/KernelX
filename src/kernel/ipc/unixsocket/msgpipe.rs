use alloc::collections::VecDeque;
use alloc::sync::Arc;
use alloc::vec::Vec;

use crate::kernel::errno::{Errno, SysResult};
use crate::kernel::event::{EpollNotifier, Event, FileEvent, WaitQueue};
use crate::kernel::mm::ubuf::UAddrSpaceBuffer;
use crate::kernel::scheduler::current;
use crate::klib::SleepLock;

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
    state: SleepLock<MessagePipeState>,
    read_notifier: Arc<EpollNotifier>,
    write_notifier: Arc<EpollNotifier>,
}

struct MessagePipeState {
    queue: MessageQueue,
    read_waiter: WaitQueue<Event>,
    write_waiter: WaitQueue<Event>,
    writer_count: u32,
    reader_count: u32,
}

impl MessagePipeState {
    fn new(capacity: usize) -> Self {
        Self {
            queue: MessageQueue::new(capacity),
            read_waiter: WaitQueue::new("MessagePipeState::read_waiter"),
            write_waiter: WaitQueue::new("MessagePipeState::write_waiter"),
            writer_count: 0,
            reader_count: 0,
        }
    }

    fn poll_event(&self, event: FileEvent, is_writer: bool) -> Option<FileEvent> {
        let want_read = event.contains(FileEvent::READ_READY) && !is_writer;
        let want_write = event.contains(FileEvent::WRITE_READY) && is_writer;
        let mut ready = FileEvent::empty();

        if want_read {
            if self.writer_count == 0 {
                ready |= FileEvent::HANG_UP;
                if !self.queue.is_empty() {
                    ready |= FileEvent::READ_READY;
                }
            } else if !self.queue.is_empty() {
                ready |= FileEvent::READ_READY;
            }
        }

        if want_write {
            if self.reader_count == 0 {
                ready |= FileEvent::HANG_UP;
            } else if self.queue.available_space() > 0 && self.queue.available_messages() > 0 {
                ready |= FileEvent::WRITE_READY;
            }
        }

        if ready.is_empty() { None } else { Some(ready) }
    }
}

impl MessagePipeInner {
    pub fn new(capacity: usize) -> Self {
        Self {
            state: SleepLock::new(MessagePipeState::new(capacity), "MessagePipeInner::state"),
            read_notifier: Arc::new(EpollNotifier::new()),
            write_notifier: Arc::new(EpollNotifier::new()),
        }
    }

    pub fn read(&self, buf: &mut [u8], blocked: bool) -> SysResult<usize> {
        if buf.is_empty() {
            return Ok(0);
        }

        loop {
            let mut state = self.state.lock();
            if let Some(msg) = state.queue.pop() {
                let copy_len = core::cmp::min(buf.len(), msg.len());
                buf[..copy_len].copy_from_slice(&msg[..copy_len]);
                state.write_waiter.wake_all(|e| e);
                drop(state);
                self.write_notifier.notify(FileEvent::WRITE_READY);
                return Ok(copy_len);
            }

            if state.writer_count == 0 {
                return Ok(0); // EOF
            }

            if !blocked {
                return Err(Errno::EAGAIN);
            }

            state.read_waiter.wait_current(Event::ReadReady);
            drop(state);

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
            let mut state = self.state.lock();
            if let Some(msg) = state.queue.pop() {
                let copy_len = core::cmp::min(ubuf.length(), msg.len());
                ubuf.write(0, &msg[..copy_len])?;
                state.write_waiter.wake_all(|e| e);
                drop(state);
                self.write_notifier.notify(FileEvent::WRITE_READY);
                return Ok(copy_len);
            }

            if state.writer_count == 0 {
                return Ok(0); // EOF
            }

            if !blocked {
                return Err(Errno::EAGAIN);
            }

            state.read_waiter.wait_current(Event::ReadReady);
            drop(state);

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
        loop {
            let mut state = self.state.lock();
            if state.reader_count == 0 {
                return Err(Errno::EPIPE);
            }

            if state.queue.can_push(buf.len()) {
                state.queue.push(Vec::from(buf));
                state.read_waiter.wake_all(|e| e);
                drop(state);
                self.read_notifier.notify(FileEvent::READ_READY);
                return Ok(buf.len());
            }

            if !blocked {
                return Err(Errno::EAGAIN);
            }

            state.write_waiter.wait_current(Event::WriteReady);
            drop(state);

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
        if self.state.lock().reader_count == 0 {
            return Err(Errno::EPIPE);
        }

        // Collect user data into a kernel buffer first
        let mut msg = Vec::with_capacity(ubuf.length());
        for kbuf in ubuf.iter() {
            let kbuf = kbuf?;
            msg.extend_from_slice(&kbuf);
        }

        loop {
            let mut state = self.state.lock();
            if state.reader_count == 0 {
                return Err(Errno::EPIPE);
            }

            if state.queue.can_push(msg.len()) {
                let len = msg.len();
                state.queue.push(msg);
                state.read_waiter.wake_all(|e| e);
                drop(state);
                self.read_notifier.notify(FileEvent::READ_READY);
                return Ok(len);
            }

            if !blocked {
                return Err(Errno::EAGAIN);
            }

            state.write_waiter.wait_current(Event::WriteReady);
            drop(state);

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

        Ok(self.state.lock().poll_event(event, is_writer))
    }

    pub fn wait_event(&self, waker: usize, event: FileEvent, is_writer: bool) -> SysResult<Option<FileEvent>> {
        let want_read = event.contains(FileEvent::READ_READY) && !is_writer;
        let want_write = event.contains(FileEvent::WRITE_READY) && is_writer;
        if !want_read && !want_write {
            return Ok(None);
        }

        let mut state = self.state.lock();
        if let Some(ready) = state.poll_event(event, is_writer) {
            return Ok(Some(ready));
        }

        if want_read {
            state.read_waiter.wait_pending(
                current::task().clone(),
                Event::Poll {
                    event: FileEvent::READ_READY,
                    waker,
                },
            );
        }

        if want_write {
            state.write_waiter.wait_pending(
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
        let mut state = self.state.lock();
        state.read_waiter.remove(current::task());
        state.write_waiter.remove(current::task());
    }

    pub fn epoll_notifier(&self, is_writer: bool) -> Arc<EpollNotifier> {
        if is_writer {
            self.write_notifier.clone()
        } else {
            self.read_notifier.clone()
        }
    }

    pub fn increment_reader_count(&self) {
        let mut state = self.state.lock();
        state.reader_count += 1;
        state.write_waiter.wake_all(|e| e);
        drop(state);
        self.write_notifier.notify(FileEvent::WRITE_READY);
    }

    pub fn decrement_reader_count(&self) {
        let has_no_reader = {
            let mut state = self.state.lock();
            debug_assert!(state.reader_count > 0);
            state.reader_count -= 1;
            let has_no_reader = state.reader_count == 0;
            if has_no_reader {
                state.write_waiter.wake_all(|e| e);
            }
            has_no_reader
        };

        if has_no_reader {
            self.write_notifier.notify(FileEvent::HANG_UP);
        }
    }

    pub fn increment_writer_count(&self) {
        let mut state = self.state.lock();
        state.writer_count += 1;
        state.read_waiter.wake_all(|e| e);
        drop(state);
        self.read_notifier.notify(FileEvent::READ_READY);
    }

    pub fn decrement_writer_count(&self) {
        let wake_event = {
            let mut state = self.state.lock();
            debug_assert!(state.writer_count > 0);
            state.writer_count -= 1;
            if state.writer_count != 0 {
                None
            } else {
                let wake_event = if !state.queue.is_empty() {
                    FileEvent::READ_READY | FileEvent::HANG_UP
                } else {
                    FileEvent::HANG_UP
                };
                state.read_waiter.wake_all(|e| match e {
                    Event::Poll { event, waker } if event.intersects(FileEvent::READ_READY) => Event::Poll {
                        event: wake_event,
                        waker,
                    },
                    _ => e,
                });
                Some(wake_event)
            }
        };

        if let Some(wake_event) = wake_event {
            self.read_notifier.notify(wake_event);
        }
    }
}
