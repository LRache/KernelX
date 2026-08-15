use alloc::sync::Arc;
use alloc::vec;
use alloc::vec::Vec;

use crate::arch;
use crate::kernel::config;
use crate::kernel::errno::{Errno, SysResult};
use crate::kernel::event::{EpollNotifier, Event, FileEvent, WaitQueue};
use crate::kernel::ipc::{KSiFields, SiCode, signum};
use crate::kernel::mm::FixedContiguousPhysPageFrame;
use crate::kernel::mm::ubuf::UAddrSpaceBuffer;
use crate::kernel::scheduler::current;
use crate::kernel::task::CapabilitySet;
use crate::klib::SleepLock;

const PIPE_BUF: usize = 4096;
const PIPE_CAPACITY: usize = arch::PGSIZE * config::PIPE_BUFFER_PAGES;
type PipeBuffer = FixedContiguousPhysPageFrame<{ config::PIPE_BUFFER_PAGES }>;

struct FIFO {
    data: PipeBuffer,
    head: usize,
    tail: usize,
    length: usize,
}

impl FIFO {
    fn new() -> Self {
        Self {
            data: PipeBuffer::alloc(),
            head: 0,
            tail: 0,
            length: 0,
        }
    }

    fn len(&self) -> usize {
        self.length
    }

    fn clear(&mut self) {
        self.head = 0;
        self.tail = 0;
        self.length = 0;
    }

    fn data_mut(&mut self) -> &mut [u8] {
        self.data.slice()
    }

    fn data(&self) -> &[u8] {
        unsafe { core::slice::from_raw_parts(self.data.ptr(), PIPE_CAPACITY) }
    }

    fn pop_front(&mut self) -> Option<u8> {
        if self.length == 0 {
            return None;
        }
        let head = self.head;
        let byte = self.data_mut()[head];
        self.head = (self.head + 1) % PIPE_CAPACITY;
        self.length -= 1;
        Some(byte)
    }

    fn pop_front_ubuf(&mut self, ubuf: &UAddrSpaceBuffer) -> SysResult<usize> {
        let n = core::cmp::min(ubuf.length(), self.length);
        if n == 0 {
            return Ok(0);
        }

        let head = self.head;
        if self.head + n <= PIPE_CAPACITY {
            ubuf.write(0, &self.data_mut()[head..head + n])?;
        } else {
            let first_part = PIPE_CAPACITY - head;
            ubuf.write(0, &self.data_mut()[head..PIPE_CAPACITY])?;
            ubuf.write(first_part, &self.data_mut()[0..n - first_part])?;
        }
        self.head = (self.head + n) % PIPE_CAPACITY;
        self.length -= n;

        // crate::kinfo!("after pop front, len={}", self.length);

        Ok(n)
    }

    fn peek_front(&self, buf: &mut [u8]) -> usize {
        let n = core::cmp::min(buf.len(), self.length);
        if n == 0 {
            return 0;
        }

        let head = self.head;
        if head + n <= PIPE_CAPACITY {
            buf[..n].copy_from_slice(&self.data()[head..head + n]);
        } else {
            let first_part = PIPE_CAPACITY - head;
            buf[..first_part].copy_from_slice(&self.data()[head..PIPE_CAPACITY]);
            buf[first_part..n].copy_from_slice(&self.data()[0..n - first_part]);
        }

        n
    }

    fn push_back(&mut self, byte: u8) -> bool {
        if self.length == PIPE_CAPACITY {
            return false;
        }
        let tail = self.tail;
        self.data_mut()[tail] = byte;
        self.tail = (tail + 1) % PIPE_CAPACITY;
        self.length += 1;
        true
    }

    fn push_back_ubuf(&mut self, ubuf: &UAddrSpaceBuffer, capacity: usize) -> SysResult<usize> {
        debug_assert!(capacity <= PIPE_CAPACITY);
        let n = core::cmp::min(ubuf.length(), capacity.saturating_sub(self.length));
        if n == 0 {
            return Ok(0);
        }

        for kbuf in ubuf.iter() {
            let kbuf = kbuf?;
            let max_to_push = core::cmp::min(kbuf.len(), capacity.saturating_sub(self.length));
            let tail = self.tail;
            if tail + max_to_push <= PIPE_CAPACITY {
                self.data_mut()[tail..tail + max_to_push].copy_from_slice(&kbuf[..max_to_push]);
                self.tail = (self.tail + max_to_push) % PIPE_CAPACITY;
            } else {
                let part1 = PIPE_CAPACITY - tail;
                self.data_mut()[tail..PIPE_CAPACITY].copy_from_slice(&kbuf[..part1]);
                let part2 = max_to_push - part1;
                self.data_mut()[0..part2].copy_from_slice(&kbuf[part1..max_to_push]);
                self.tail = part2;
            }
            self.length += max_to_push;
            if self.length >= capacity {
                break;
            }
        }

        Ok(n)
    }
}

pub struct PipeInner {
    state: SleepLock<PipeState>,
    read_notifier: Arc<EpollNotifier>,
    write_notifier: Arc<EpollNotifier>,
}

struct PipeState {
    fifo: FIFO,
    read_waiter: WaitQueue<Event>,
    write_waiter: WaitQueue<Event>,
    capacity: usize,
    writer_count: u32,
    reader_count: u32,
}

impl PipeState {
    fn new(capacity: usize) -> Self {
        Self {
            fifo: FIFO::new(),
            read_waiter: WaitQueue::new("PipeState::read_waiter"),
            write_waiter: WaitQueue::new("PipeState::write_waiter"),
            capacity,
            writer_count: 0,
            reader_count: 0,
        }
    }

    fn poll_event(&self, event: FileEvent, writable: bool) -> Option<FileEvent> {
        let want_read = event.contains(FileEvent::READ_READY) && !writable;
        let want_write = event.contains(FileEvent::WRITE_READY) && writable;
        let mut ready = FileEvent::empty();

        if want_read {
            if self.writer_count == 0 {
                ready |= FileEvent::HANG_UP;
                if self.fifo.len() > 0 {
                    ready |= FileEvent::READ_READY;
                }
            } else if self.fifo.len() > 0 {
                ready |= FileEvent::READ_READY;
            }
        }

        if want_write {
            if self.reader_count == 0 {
                ready |= FileEvent::HANG_UP;
            } else if self.fifo.len() < self.capacity {
                ready |= FileEvent::WRITE_READY;
            }
        }

        if ready.is_empty() { None } else { Some(ready) }
    }
}

impl PipeInner {
    pub fn new(capacity: usize) -> Self {
        Self {
            state: SleepLock::new(PipeState::new(capacity), "PipeInner::state"),
            read_notifier: Arc::new(EpollNotifier::new()),
            write_notifier: Arc::new(EpollNotifier::new()),
        }
    }

    pub fn read(&self, buf: &mut [u8], blocked: bool) -> SysResult<usize> {
        if buf.len() == 0 {
            return Ok(0);
        }

        // Phase 1: wait until at least one byte is available
        let notify_write_ready;
        loop {
            let mut state = self.state.lock();
            if state.fifo.len() > 0 {
                let was_write_ready = state.fifo.len() < state.capacity;
                buf[0] = state.fifo.pop_front().unwrap();
                notify_write_ready = !was_write_ready;
                break;
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

        // Phase 2: drain as much as is immediately available
        let mut total_read = 1;
        while total_read < buf.len() {
            let mut state = self.state.lock();
            if state.fifo.len() == 0 {
                break;
            }
            let to_read = core::cmp::min(buf.len() - total_read, state.fifo.len());
            for _ in 0..to_read {
                buf[total_read] = state.fifo.pop_front().unwrap();
                total_read += 1;
            }
        }

        self.state.lock().write_waiter.wake_all(|e| e);
        if notify_write_ready {
            self.write_notifier.notify(FileEvent::WRITE_READY);
        }

        Ok(total_read)
    }

    pub fn read_to_user(&self, ubuf: &UAddrSpaceBuffer, blocked: bool) -> SysResult<usize> {
        if ubuf.length() == 0 {
            return Ok(0);
        }

        loop {
            let mut state = self.state.lock();

            if state.fifo.len() > 0 {
                let was_write_ready = state.fifo.len() < state.capacity;
                let r = state.fifo.pop_front_ubuf(ubuf);
                if r.as_ref().map_or(false, |n| *n > 0) {
                    state.write_waiter.wake_all(|e| e);
                    drop(state);
                    if !was_write_ready {
                        self.write_notifier.notify(FileEvent::WRITE_READY);
                    }
                }
                return r;
            }

            if state.writer_count == 0 {
                return Ok(0); // EOF: no writers left
            }

            if !blocked {
                return Err(Errno::EAGAIN);
            }

            // Wait for data
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
        let mut state = self.state.lock();
        if state.reader_count == 0 {
            drop(state);
            let _ = current::pcb().send_signal(signum::SIGPIPE, SiCode::EMPTY, 0, KSiFields::Empty, None);
            return Err(Errno::EPIPE);
        }

        if buf.len() > PIPE_BUF {
            // Large write (> PIPE_BUF): write as much as fits, non-atomic
            loop {
                if state.reader_count == 0 {
                    drop(state);
                    let _ = current::pcb().send_signal(signum::SIGPIPE, SiCode::EMPTY, 0, KSiFields::Empty, None);
                    return Err(Errno::EPIPE);
                }
                if state.fifo.len() < state.capacity {
                    break;
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
                state = self.state.lock();
            }

            let to_write = core::cmp::min(buf.len(), state.capacity.saturating_sub(state.fifo.len()));
            for i in 0..to_write {
                state.fifo.push_back(buf[i]);
            }
            state.read_waiter.wake_all(|e| e);
            drop(state);
            self.read_notifier.notify(FileEvent::READ_READY);
            Ok(to_write)
        } else {
            // Small write (<= PIPE_BUF): must be atomic, wait until space available
            loop {
                if state.reader_count == 0 {
                    drop(state);
                    let _ = current::pcb().send_signal(signum::SIGPIPE, SiCode::EMPTY, 0, KSiFields::Empty, None);
                    return Err(Errno::EPIPE);
                }
                let cap = state.capacity;
                if cap - state.fifo.len() >= buf.len() {
                    break;
                }
                if !blocked {
                    return Err(Errno::EAGAIN);
                }
                // Buffer is full, wait for space
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
                state = self.state.lock();
            }

            for c in buf {
                state.fifo.push_back(*c);
            }
            state.read_waiter.wake_all(|e| e);
            drop(state);
            self.read_notifier.notify(FileEvent::READ_READY);
            Ok(buf.len())
        }
    }

    pub fn peek(&self, len: usize, blocked: bool) -> SysResult<Vec<u8>> {
        if len == 0 {
            return Ok(Vec::new());
        }

        loop {
            let mut state = self.state.lock();

            if state.fifo.len() > 0 {
                let n = core::cmp::min(len, state.fifo.len());
                let mut buf = vec![0u8; n];
                state.fifo.peek_front(&mut buf);
                return Ok(buf);
            }

            if state.writer_count == 0 {
                return Ok(Vec::new());
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

    pub fn write_from_user(&self, ubuf: &UAddrSpaceBuffer, blocked: bool) -> SysResult<usize> {
        let mut state = self.state.lock();
        if state.reader_count == 0 {
            drop(state);
            let _ = current::pcb().send_signal(signum::SIGPIPE, SiCode::EMPTY, 0, KSiFields::Empty, None);
            return Err(Errno::EPIPE);
        }

        if ubuf.length() > PIPE_BUF {
            // Large write (> PIPE_BUF): write as much as fits, non-atomic
            loop {
                if state.reader_count == 0 {
                    drop(state);
                    let _ = current::pcb().send_signal(signum::SIGPIPE, SiCode::EMPTY, 0, KSiFields::Empty, None);
                    return Err(Errno::EPIPE);
                }
                if state.fifo.len() < state.capacity {
                    break;
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
                state = self.state.lock();
            }

            let capacity = state.capacity;
            let n = state.fifo.push_back_ubuf(ubuf, capacity)?;
            state.read_waiter.wake_all(|e| e);
            drop(state);
            self.read_notifier.notify(FileEvent::READ_READY);
            Ok(n)
        } else {
            // Small write: atomic, wait until enough space available
            loop {
                if state.reader_count == 0 {
                    drop(state);
                    let _ = current::pcb().send_signal(signum::SIGPIPE, SiCode::EMPTY, 0, KSiFields::Empty, None);
                    return Err(Errno::EPIPE);
                }
                let cap = state.capacity;
                if cap - state.fifo.len() >= ubuf.length() {
                    break;
                }
                if !blocked {
                    return Err(Errno::EAGAIN);
                }
                // Buffer is full, wait for space
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
                state = self.state.lock();
            }

            let capacity = state.capacity;
            state.fifo.push_back_ubuf(ubuf, capacity)?;
            state.read_waiter.wake_all(|e| e);
            drop(state);
            self.read_notifier.notify(FileEvent::READ_READY);
            Ok(ubuf.length())
        }
    }

    pub fn poll_event(&self, event: FileEvent, writable: bool) -> SysResult<Option<FileEvent>> {
        let want_read = event.contains(FileEvent::READ_READY) && !writable;
        let want_write = event.contains(FileEvent::WRITE_READY) && writable;
        if !want_read && !want_write {
            return Ok(None);
        }

        Ok(self.state.lock().poll_event(event, writable))
    }

    pub fn wait_event(&self, waker: usize, event: FileEvent, writable: bool) -> SysResult<Option<FileEvent>> {
        let want_read = event.contains(FileEvent::READ_READY) && !writable;
        let want_write = event.contains(FileEvent::WRITE_READY) && writable;
        if !want_read && !want_write {
            return Ok(None);
        }

        let mut state = self.state.lock();
        if let Some(ready) = state.poll_event(event, writable) {
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

    pub fn epoll_notifier(&self, writable: bool) -> Arc<EpollNotifier> {
        if writable {
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
            // Wake blocked writers so they can return EPIPE
            self.write_notifier.notify(FileEvent::HANG_UP);
        }
        self.clear_if_unused();
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
                let wake_event = if state.fifo.len() > 0 {
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
        self.clear_if_unused();
    }

    fn clear_if_unused(&self) {
        let mut state = self.state.lock();
        if state.reader_count == 0 && state.writer_count == 0 {
            state.fifo.clear();
        }
    }

    pub fn has_readers(&self) -> bool {
        self.state.lock().reader_count > 0
    }

    pub fn wait_for_reader(&self) -> SysResult<()> {
        loop {
            let mut state = self.state.lock();
            if state.reader_count > 0 {
                return Ok(());
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

    pub fn wait_for_writer(&self) -> SysResult<()> {
        loop {
            let mut state = self.state.lock();
            if state.writer_count > 0 {
                return Ok(());
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

    pub fn get_capacity(&self) -> usize {
        self.state.lock().capacity
    }

    pub fn read_available(&self) -> usize {
        self.state.lock().fifo.len()
    }

    pub fn set_capacity(&self, size: usize) -> SysResult<usize> {
        if (size as isize) < 0 {
            return Err(Errno::EINVAL);
        }

        let aligned = if size == 0 {
            arch::PGSIZE
        } else {
            size.div_ceil(arch::PGSIZE) * arch::PGSIZE
        };

        let mut state = self.state.lock();
        let used = state.fifo.len();
        if aligned < used {
            return Err(Errno::EBUSY);
        }

        if !current::capable(CapabilitySet::SYS_RESOURCE) {
            return Err(Errno::EPERM);
        }

        if aligned > PIPE_CAPACITY {
            return Err(Errno::EINVAL);
        }

        state.capacity = aligned;
        // Capacity changes may unblock writers waiting for room.
        state.write_waiter.wake_all(|e| e);
        drop(state);
        self.write_notifier.notify(FileEvent::WRITE_READY);
        Ok(aligned)
    }
}
