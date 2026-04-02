use crate::arch;
use crate::kernel::config;
use crate::kernel::errno::{Errno, SysResult};
use crate::kernel::event::{Event, FileEvent, PollEventSet, WaitQueue};
use crate::kernel::mm::page;
use crate::kernel::mm::ubuf::UAddrSpaceBuffer;
use crate::kernel::scheduler::current;
use crate::klib::{SleepLock, SpinLock};

const PIPE_CAPACITY: usize = arch::PGSIZE * config::PIPE_BUFFER_PAGES;

struct FIFO {
    data: * mut [u8; PIPE_CAPACITY],
    head: usize,
    tail: usize,
    length: usize
}

impl FIFO {
    fn new() -> Self {
        let data = page::alloc_contiguous(config::PIPE_BUFFER_PAGES) as * mut [u8; PIPE_CAPACITY];
        Self {
            data,
            head: 0,
            tail: 0,
            length: 0,
        }
    }

    fn len(&self) -> usize {
        self.length
    }

    fn data_mut(&mut self) -> &mut [u8; PIPE_CAPACITY] {
        unsafe { &mut *self.data }
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

    fn push_back(&mut self, byte: u8) -> bool {
        if self.length == PIPE_CAPACITY {
            return false
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
        // kinfo!("head={}, tail={}, ubuf.len={}", self.head, self.tail, ubuf.length());

        Ok(n)
    }
}

unsafe impl Send for FIFO {}

pub struct PipeInner {
    fifo: SleepLock<FIFO>,
    read_waiter: SpinLock<WaitQueue<Event>>,
    write_waiter: SpinLock<WaitQueue<Event>>,
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
                Event::ReadReady => {},
                Event::Signal => return Err(Errno::EINTR),
                _ => unreachable!()
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
                Event::ReadReady => {},
                Event::Signal => return Err(Errno::EINTR),
                _ => unreachable!()
            }
        }
    }

    pub fn write(&self, buf: &[u8], blocked: bool) -> SysResult<usize> {
        if *self.reader_count.lock() == 0 {
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
                    Event::WriteReady => {},
                    Event::Signal => return Err(Errno::EINTR),
                    _ => unreachable!()
                }
            }

            for c in buf {
                fifo.push_back(*c);
            }
            drop(fifo);
            self.read_waiter.lock().wake_all(|e| e);
            Ok(buf.len())
        }
    }

    pub fn write_from_user(&self, ubuf: &UAddrSpaceBuffer, blocked: bool) -> SysResult<usize> {
        if *self.reader_count.lock() == 0 {
            return Err(Errno::EPIPE);
        }

        let cap = *self.capacity.lock();

        if ubuf.length() >= cap {
            // Large write: write as much as fits, non-atomic
            let n = self.fifo.lock().push_back_ubuf(ubuf)?;
            self.read_waiter.lock().wake_all(|e| e);
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
                    Event::WriteReady => {},
                    Event::Signal => return Err(Errno::EINTR),
                    _ => unreachable!()
                }
            }

            fifo.push_back_ubuf(ubuf)?;
            drop(fifo);
            self.read_waiter.lock().wake_all(|e| e);
            Ok(ubuf.length())
        }
    }

    pub fn wait_event(&self, waker: usize, event: PollEventSet, writable: bool) -> SysResult<Option<FileEvent>> {
        // Pipe read end only handles POLLIN; write end only handles POLLOUT
        if event.contains(PollEventSet::POLLIN) && writable {
            return Ok(None);
        }
        if event.contains(PollEventSet::POLLOUT) && !writable {
            return Ok(None);
        }

        let fifo = self.fifo.lock();

        if event.contains(PollEventSet::POLLIN) {
            if *self.writer_count.lock() == 0 {
                // All writers gone: report HangUp regardless of data left
                return Ok(Some(FileEvent::HangUp));
            }
            if fifo.len() > 0 {
                return Ok(Some(FileEvent::ReadReady));
            }
            self.read_waiter.lock().wait(current::task().clone(), Event::Poll { event: FileEvent::ReadReady, waker });
        }

        if event.contains(PollEventSet::POLLOUT) {
            if *self.reader_count.lock() == 0 {
                // All readers gone: write end should get HangUp (caller maps to EPIPE)
                return Ok(Some(FileEvent::HangUp));
            }
            if fifo.len() < *self.capacity.lock() {
                return Ok(Some(FileEvent::WriteReady));
            }
            self.write_waiter.lock().wait(current::task().clone(), Event::Poll { event: FileEvent::WriteReady, waker });
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
        let mut reader_count = self.reader_count.lock();
        debug_assert!(*reader_count > 0);
        *reader_count -= 1;
        if *reader_count == 0 {
            // Wake blocked writers so they can return EPIPE
            self.write_waiter.lock().wake_all(|e| e);
        }
    }

    pub fn increment_writer_count(&self) {
        *self.writer_count.lock() += 1;
    }

    pub fn decrement_writer_count(&self) {
        let mut writer_count = self.writer_count.lock();
        debug_assert!(*writer_count > 0);
        *writer_count -= 1;
        if *writer_count == 0 {
            self.read_waiter.lock().wake_all(|e| {
                match e {
                    Event::Poll{ event: FileEvent::ReadReady, waker } => { Event::Poll{event: FileEvent::HangUp, waker} },
                    _ => e
                }
            }); // Wake up readers to notify them of EOF
        }
    }
}  