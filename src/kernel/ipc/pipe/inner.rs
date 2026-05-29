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
use crate::klib::{SleepLock, SpinLock};

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

    fn push_back_ubuf(&mut self, ubuf: &UAddrSpaceBuffer) -> SysResult<usize> {
        let n = core::cmp::min(ubuf.length(), PIPE_CAPACITY - self.length);
        if n == 0 {
            return Ok(0);
        }

        for kbuf in ubuf.iter() {
            let kbuf = kbuf?;
            let max_to_push = core::cmp::min(kbuf.len(), PIPE_CAPACITY - self.length);
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
        }

        Ok(n)
    }
}

pub struct PipeInner {
    fifo: SleepLock<FIFO>,
    read_waiter: SpinLock<WaitQueue<Event>>,
    write_waiter: SpinLock<WaitQueue<Event>>,
    read_notifier: Arc<EpollNotifier>,
    write_notifier: Arc<EpollNotifier>,
    capacity: SpinLock<usize>,
    writer_count: SpinLock<u32>,
    reader_count: SpinLock<u32>,
}

impl PipeInner {
    pub fn new(capacity: usize) -> Self {
        Self {
            fifo: SleepLock::new(FIFO::new(), "PipeInner::fifo"),
            read_waiter: SpinLock::new(WaitQueue::new(), "PipeInner::read_waiter"),
            write_waiter: SpinLock::new(WaitQueue::new(), "PipeInner::write_waiter"),
            read_notifier: Arc::new(EpollNotifier::new()),
            write_notifier: Arc::new(EpollNotifier::new()),
            capacity: SpinLock::new(capacity, "PipeInner::capacity"),
            writer_count: SpinLock::new(0, "PipeInner::writer_count"),
            reader_count: SpinLock::new(0, "PipeInner::reader_count"),
        }
    }

    pub fn read(&self, buf: &mut [u8], blocked: bool) -> SysResult<usize> {
        if buf.len() == 0 {
            return Ok(0);
        }

        // Phase 1: wait until at least one byte is available
        loop {
            let mut fifo = self.fifo.lock();
            if fifo.len() > 0 {
                buf[0] = fifo.pop_front().unwrap();
                drop(fifo);
                self.write_waiter.lock().wake_all(|e| e);
                self.write_notifier.notify(FileEvent::WRITE_READY);
                break;
            }
            if *self.writer_count.lock() == 0 {
                return Ok(0); // EOF
            }
            drop(fifo);
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

        // Phase 2: drain as much as is immediately available
        let mut total_read = 1;
        while total_read < buf.len() {
            let mut fifo = self.fifo.lock();
            if fifo.len() == 0 {
                break;
            }
            let to_read = core::cmp::min(buf.len() - total_read, fifo.len());
            for _ in 0..to_read {
                buf[total_read] = fifo.pop_front().unwrap();
                total_read += 1;
            }
            drop(fifo);
            self.write_waiter.lock().wake_all(|e| e);
            self.write_notifier.notify(FileEvent::WRITE_READY);
        }

        Ok(total_read)
    }

    pub fn read_to_user(&self, ubuf: &UAddrSpaceBuffer, blocked: bool) -> SysResult<usize> {
        if ubuf.length() == 0 {
            return Ok(0);
        }

        loop {
            let mut fifo = self.fifo.lock();

            if fifo.len() > 0 {
                let r = fifo.pop_front_ubuf(ubuf);
                drop(fifo);
                self.write_waiter.lock().wake_all(|e| e);
                self.write_notifier.notify(FileEvent::WRITE_READY);
                return r;
            }

            if *self.writer_count.lock() == 0 {
                return Ok(0); // EOF: no writers left
            }

            drop(fifo);

            if !blocked {
                return Err(Errno::EAGAIN);
            }

            // Wait for data
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
            let _ = current::pcb().send_signal(signum::SIGPIPE, SiCode::EMPTY, 0, KSiFields::Empty, None);
            return Err(Errno::EPIPE);
        }

        let cap = *self.capacity.lock();

        if buf.len() >= cap {
            // Large write (> PIPE_BUF): write as much as fits, non-atomic
            let mut fifo = self.fifo.lock();
            let to_write = core::cmp::min(buf.len(), PIPE_CAPACITY - fifo.len());
            for i in 0..to_write {
                fifo.push_back(buf[i]);
            }
            drop(fifo);
            self.read_waiter.lock().wake_all(|e| e);
            self.read_notifier.notify(FileEvent::READ_READY);
            Ok(to_write)
        } else {
            // Small write (<= PIPE_BUF): must be atomic, wait until space available
            let mut fifo;
            loop {
                if *self.reader_count.lock() == 0 {
                    return Err(Errno::EPIPE);
                }
                let cap = *self.capacity.lock();
                fifo = self.fifo.lock();
                if cap - fifo.len() >= buf.len() {
                    break;
                }
                drop(fifo);
                if !blocked {
                    return Err(Errno::EAGAIN);
                }
                // Buffer is full, wait for space
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

            for c in buf {
                fifo.push_back(*c);
            }
            drop(fifo);
            self.read_waiter.lock().wake_all(|e| e);
            self.read_notifier.notify(FileEvent::READ_READY);
            Ok(buf.len())
        }
    }

    pub fn peek(&self, len: usize, blocked: bool) -> SysResult<Vec<u8>> {
        if len == 0 {
            return Ok(Vec::new());
        }

        loop {
            let fifo = self.fifo.lock();

            if fifo.len() > 0 {
                let n = core::cmp::min(len, fifo.len());
                let mut buf = vec![0u8; n];
                fifo.peek_front(&mut buf);
                return Ok(buf);
            }

            if *self.writer_count.lock() == 0 {
                return Ok(Vec::new());
            }

            drop(fifo);

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

    pub fn write_from_user(&self, ubuf: &UAddrSpaceBuffer, blocked: bool) -> SysResult<usize> {
        if *self.reader_count.lock() == 0 {
            let _ = current::pcb().send_signal(signum::SIGPIPE, SiCode::EMPTY, 0, KSiFields::Empty, None);
            return Err(Errno::EPIPE);
        }

        let cap = *self.capacity.lock();

        if ubuf.length() >= cap {
            // Large write: write as much as fits, non-atomic
            let n = self.fifo.lock().push_back_ubuf(ubuf)?;
            self.read_waiter.lock().wake_all(|e| e);
            self.read_notifier.notify(FileEvent::READ_READY);
            Ok(n)
        } else {
            // Small write: atomic, wait until enough space available
            let mut fifo;
            loop {
                if *self.reader_count.lock() == 0 {
                    return Err(Errno::EPIPE);
                }
                let cap = *self.capacity.lock();
                fifo = self.fifo.lock();
                if cap - fifo.len() >= ubuf.length() {
                    break;
                }
                drop(fifo);
                if !blocked {
                    return Err(Errno::EAGAIN);
                }
                // Buffer is full, wait for space
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

            fifo.push_back_ubuf(ubuf)?;
            drop(fifo);
            self.read_waiter.lock().wake_all(|e| e);
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

        let fifo = self.fifo.lock();
        let mut ready = FileEvent::empty();

        if want_read {
            if *self.writer_count.lock() == 0 {
                ready |= FileEvent::HANG_UP;
                if fifo.len() > 0 {
                    ready |= FileEvent::READ_READY;
                }
            } else if fifo.len() > 0 {
                ready |= FileEvent::READ_READY;
            }
        }

        if want_write {
            if *self.reader_count.lock() == 0 {
                // All readers gone: write end should get HangUp (caller maps to EPIPE)
                ready |= FileEvent::HANG_UP;
            } else if fifo.len() < *self.capacity.lock() {
                ready |= FileEvent::WRITE_READY;
            }
        }

        if !ready.is_empty() {
            return Ok(Some(ready));
        }

        Ok(None)
    }

    pub fn wait_event(&self, waker: usize, event: FileEvent, writable: bool) -> SysResult<Option<FileEvent>> {
        let want_read = event.contains(FileEvent::READ_READY) && !writable;
        let want_write = event.contains(FileEvent::WRITE_READY) && writable;
        if !want_read && !want_write {
            return Ok(None);
        }

        if let Some(ready) = self.poll_event(event, writable)? {
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

    pub fn epoll_notifier(&self, writable: bool) -> Arc<EpollNotifier> {
        if writable {
            self.write_notifier.clone()
        } else {
            self.read_notifier.clone()
        }
    }

    pub fn increment_reader_count(&self) {
        let mut waiter = self.write_waiter.lock();
        *self.reader_count.lock() += 1;
        waiter.wake_all(|e| e);
        self.write_notifier.notify(FileEvent::WRITE_READY);
    }

    pub fn decrement_reader_count(&self) {
        let has_no_reader = {
            let mut reader_count = self.reader_count.lock();
            debug_assert!(*reader_count > 0);
            *reader_count -= 1;
            *reader_count == 0
        };

        if has_no_reader {
            // Wake blocked writers so they can return EPIPE
            self.write_waiter.lock().wake_all(|e| e);
            self.write_notifier.notify(FileEvent::HANG_UP);
        }
        self.clear_if_unused();
    }

    pub fn increment_writer_count(&self) {
        let mut waiter = self.read_waiter.lock();
        *self.writer_count.lock() += 1;
        waiter.wake_all(|e| e);
        self.read_notifier.notify(FileEvent::READ_READY);
    }

    pub fn decrement_writer_count(&self) {
        let has_no_writer = {
            let mut writer_count = self.writer_count.lock();
            debug_assert!(*writer_count > 0);
            *writer_count -= 1;
            *writer_count == 0
        };

        if has_no_writer {
            let wake_event = if self.fifo.lock().len() > 0 {
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
            }); // Wake up readers to notify them of EOF
            self.read_notifier.notify(wake_event);
        }
        self.clear_if_unused();
    }

    fn clear_if_unused(&self) {
        let reader_count = self.reader_count.lock();
        let writer_count = self.writer_count.lock();
        if *reader_count == 0 && *writer_count == 0 {
            self.fifo.lock().clear();
        }
    }

    pub fn has_readers(&self) -> bool {
        *self.reader_count.lock() > 0
    }

    pub fn wait_for_reader(&self) -> SysResult<()> {
        loop {
            let mut waiter = self.write_waiter.lock();
            if *self.reader_count.lock() > 0 {
                return Ok(());
            }
            waiter.wait_current(Event::WriteReady);
            drop(waiter);

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
            let mut waiter = self.read_waiter.lock();
            if *self.writer_count.lock() > 0 {
                return Ok(());
            }
            waiter.wait_current(Event::ReadReady);
            drop(waiter);

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
        *self.capacity.lock()
    }

    pub fn read_available(&self) -> usize {
        self.fifo.lock().len()
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

        let used = self.fifo.lock().len();
        if aligned < used {
            return Err(Errno::EBUSY);
        }

        if !current::capable(CapabilitySet::SYS_RESOURCE) {
            return Err(Errno::EPERM);
        }

        if aligned > PIPE_CAPACITY {
            return Err(Errno::EINVAL);
        }

        *self.capacity.lock() = aligned;
        // Capacity changes may unblock writers waiting for room.
        self.write_waiter.lock().wake_all(|e| e);
        self.write_notifier.notify(FileEvent::WRITE_READY);
        Ok(aligned)
    }
}
