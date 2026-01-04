use crate::arch;
use crate::kernel::config;
use crate::kernel::errno::{Errno, SysResult};
use crate::kernel::event::{Event, FileEvent, PollEventSet, WaitQueue};
use crate::kernel::mm::page;
use crate::kernel::mm::ubuf::UAddrSpaceBuffer;
use crate::kernel::scheduler::current;
use crate::klib::SpinLock;

const PIPE_CAPACITY: usize = arch::PGSIZE * config::PIPE_BUFFER_PAGES;

struct FIFO {
    data: * mut [u8; PIPE_CAPACITY],
    head: usize,
    tail: usize,
}

impl FIFO {
    fn new() -> Self {
        let data = page::alloc_contiguous(config::PIPE_BUFFER_PAGES) as * mut [u8; PIPE_CAPACITY];
        Self {
            data,
            head: 0,
            tail: 0,
        }
    }

    fn is_empty(&self) -> bool {
        self.head == self.tail
    }

    fn len(&self) -> usize {
        if self.head >= self.tail {
            self.head - self.tail
        } else {
            PIPE_CAPACITY - (self.tail - self.head)
        }
    }

    fn data_mut(&mut self) -> &mut [u8; PIPE_CAPACITY] {
        unsafe { &mut *self.data }
    }

    fn pop_front(&mut self) -> Option<u8> {
        if self.head == self.tail {
            return None;
        }
        let head = self.head;
        let byte = self.data_mut()[head];
        self.head = (self.head + 1) % PIPE_CAPACITY;
        Some(byte)
    }

    fn pop_front_ubuf(&mut self, ubuf: &UAddrSpaceBuffer) -> SysResult<usize> {
        let n = core::cmp::min(ubuf.length(), self.len());
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
        
        Ok(n)
    }

    fn push_back(&mut self, byte: u8) {
        let tail = self.tail;
        self.data_mut()[tail] = byte;
        self.tail = (tail + 1) % PIPE_CAPACITY;
    }

    fn push_back_ubuf(&mut self, ubuf: &UAddrSpaceBuffer) -> SysResult<usize> {
        let n = core::cmp::min(ubuf.length(), PIPE_CAPACITY - self.len());
        if n == 0 {
            return Ok(0);
        }

        for kbuf in ubuf.iter() {
            let kbuf = kbuf?;
            let max_to_push = core::cmp::min(kbuf.len(), PIPE_CAPACITY - self.len());
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
        }

        Ok(n)
    }
}

unsafe impl Send for FIFO {}

pub struct PipeInner {
    fifo: SpinLock<FIFO>,
    read_waiter: SpinLock<WaitQueue<Event>>,
    write_waiter: SpinLock<WaitQueue<Event>>,
    capacity: SpinLock<usize>,
    writer_count: SpinLock<u32>,
}

impl PipeInner {
    pub fn new(capacity: usize) -> Self {
        Self {
            fifo: SpinLock::new(FIFO::new()),
            read_waiter: SpinLock::new(WaitQueue::new()),
            write_waiter: SpinLock::new(WaitQueue::new()),
            capacity: SpinLock::new(capacity),
            writer_count: SpinLock::new(0),
        }
    }

    pub fn read(&self, buf: &mut [u8], blocked: bool) -> SysResult<usize> {
        if buf.len() == 0 {
            return Ok(0);
        }
        
        let mut total_read = 0;

        {
            let mut fifo = self.fifo.lock();
            match fifo.pop_front() {
                Some(byte) => {
                    buf[0] = byte;
                    total_read += 1;
                },
                None => {
                    if *self.writer_count.lock() == 0 {
                        return Ok(0); // No writers left, return 0 to indicate EOF
                    } else {
                        drop(fifo);
                        if !blocked {
                            return Ok(0);
                        }
                        // If buffer is empty, wait for data
                        self.read_waiter.lock().wait_current(Event::ReadReady);
                        current::schedule();
                    }
                }
            }
        }
        
        while total_read < buf.len() {
            let mut fifo = self.fifo.lock();
            if fifo.is_empty() {
                break;
            }
            
            let to_read = core::cmp::min(buf.len() - total_read, fifo.len());
            for _ in 0..to_read {
                if let Some(byte) = fifo.pop_front() {
                    buf[total_read] = byte;
                    total_read += 1;
                }
            }

            self.write_waiter.lock().wake_all(|e| e); // Wake up writers waiting for space
        }

        Ok(total_read)
    }

    pub fn read_to_user(&self, ubuf: &UAddrSpaceBuffer, blocked: bool) -> SysResult<usize> {
        if ubuf.length() == 0 {
            return Ok(0);
        }

        let mut fifo= loop {
            let fifo = self.fifo.lock();
            if !fifo.is_empty() {
                break fifo;
            } else {
                if *self.writer_count.lock() == 0 {
                    return Ok(0); // No writers left, return 0 to indicate EOF
                }
                if !blocked {
                    return Ok(0);
                }
                // If buffer is empty, wait for data
                self.read_waiter.lock().wait_current(Event::ReadReady);
                
                drop(fifo);
                current::schedule();

                match current::task().take_wakeup_event().unwrap() {
                    Event::ReadReady => {},
                    Event::Signal => return Err(Errno::EINTR),
                    _ => unreachable!()
                }
            }
        };

        fifo.pop_front_ubuf(ubuf)
    }

    pub fn write(&self, buf: &[u8], blocked: bool) -> SysResult<usize> {
        let cap = self.capacity.lock();

        if buf.len() >= *cap {
            let mut fifo = self.fifo.lock();
            let to_write = buf.len() - fifo.len();
            for i in 0..to_write {
                fifo.push_back(buf[i]);
            }
            self.read_waiter.lock().wake_all(|e| e); // Wake up readers waiting

            Ok(to_write)
        } else {
            drop(cap);
            let mut fifo;
            loop {
                let cap = self.capacity.lock();
                fifo = self.fifo.lock();
                if *cap - fifo.len() >= buf.len() {
                    break;
                } else {
                    drop(cap);
                    drop(fifo);
                    if !blocked {
                        return Ok(0);
                    }
                    // If buffer is full, wait for space
                    self.write_waiter.lock().wait_current(Event::WriteReady);
                    current::schedule();

                    match current::task().take_wakeup_event().unwrap() {
                        Event::WriteReady => {},
                        Event::Signal => return Err(Errno::EINTR),
                        _ => unreachable!()
                    }
                }
            }

            for c in buf {
                fifo.push_back(*c);
            }

            self.read_waiter.lock().wake_all(|e| e); // Wake up readers waiting for data

            Ok(buf.len())
        }
    }

    pub fn write_from_user(&self, ubuf: &UAddrSpaceBuffer, blocked: bool) -> SysResult<usize> {
        let cap = self.capacity.lock();

        if ubuf.length() >= *cap {
            let n = self.fifo.lock().push_back_ubuf(ubuf)?;
            self.read_waiter.lock().wake_all(|e| e); // Wake up readers waiting
            Ok(n)
        } else {
            drop(cap);
            let mut fifo;
            loop {
                let cap = self.capacity.lock();
                fifo = self.fifo.lock();
                if *cap - fifo.len() >= ubuf.length() {
                    break;
                } else {
                    drop(cap);
                    drop(fifo);
                    if !blocked {
                        return Ok(0);
                    }
                    // If buffer is full, wait for space
                    self.write_waiter.lock().wait_current(Event::WriteReady);
                    current::schedule();

                    match current::task().take_wakeup_event().unwrap() {
                        Event::WriteReady => {},
                        Event::Signal => return Err(Errno::EINTR),
                        _ => unreachable!()
                    }
                }
            }

            fifo.push_back_ubuf(ubuf)?;

            self.read_waiter.lock().wake_all(|e| e); // Wake up readers waiting for data

            Ok(ubuf.length())
        }
    }

    pub fn wait_event(&self, waker: usize, event: PollEventSet, writable: bool) -> SysResult<Option<FileEvent>> {
        if event.contains(PollEventSet::POLLIN) && writable {
            return Ok(None);
        }

        if event.contains(PollEventSet::POLLOUT) && !writable {
            return Ok(None);
        }
        
        let fifo = self.fifo.lock();
        if event.contains(PollEventSet::POLLIN) {
            if !fifo.is_empty() && !writable {
                return Ok(Some(FileEvent::ReadReady));
            } else {
                if *self.writer_count.lock() == 0 {
                    return Ok(Some(FileEvent::HangUp)); // No writers left, indicate EOF
                }
                self.read_waiter.lock().wait(current::task().clone(), Event::Poll{event: FileEvent::ReadReady, waker});
            }
        }

        if event.contains(PollEventSet::POLLOUT) {
            if fifo.len() < *self.capacity.lock() {
                return Ok(Some(FileEvent::WriteReady));
            } else {
                self.write_waiter.lock().wait(current::task().clone(), Event::Poll{event: FileEvent::WriteReady, waker});
            }
        }

        Ok(None)
    }

    pub fn wait_event_cancel(&self) {
        self.read_waiter.lock().remove(current::task());
        self.write_waiter.lock().remove(current::task());
    }

    // pub fn increment_reader_count(&self) {
    //     *self.reader_count.lock() += 1;
    // }

    pub fn increment_writer_count(&self) {
        *self.writer_count.lock() += 1;
    }

    pub fn decrement_writer_count(&self) {
        let mut writer_count = self.writer_count.lock();
        assert!(*writer_count > 0);
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