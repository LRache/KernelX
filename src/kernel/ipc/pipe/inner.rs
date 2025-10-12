use alloc::collections::VecDeque;
use spin::Mutex;

use crate::kernel::errno::SysResult;
use crate::kernel::event::{Event, PollEvent, PollEventSet, WaitQueue};
use crate::kernel::scheduler::current;

pub struct PipeInner {
    buffer: Mutex<VecDeque<u8>>,
    read_waiter: Mutex<WaitQueue<Event>>,
    write_waiter: Mutex<WaitQueue<Event>>,
    capacity: Mutex<usize>,
    writer_count: Mutex<u32>,
}

impl PipeInner {
    pub fn new(capacity: usize) -> Self {
        Self {
            buffer: Mutex::new(VecDeque::with_capacity(capacity)),
            read_waiter: Mutex::new(WaitQueue::new()),
            write_waiter: Mutex::new(WaitQueue::new()),
            capacity: Mutex::new(capacity),
            writer_count: Mutex::new(0),
        }
    }

    pub fn read(&self, buf: &mut [u8]) -> SysResult<usize> {
        if buf.len() == 0 {
            return Ok(0);
        }
        
        let mut total_read = 0;

        {
            let mut fifo = self.buffer.lock();
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
                        self.read_waiter.lock().wait_current(Event::PipeReadReady);
                        current::schedule();
                    }
                }
            }
        }
        
        while total_read < buf.len() {
            let mut fifo = self.buffer.lock();
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

    pub fn write(&self, buf: &[u8]) -> SysResult<usize> {
        let mut total_written = 0;
        // kinfo!("Pipe write: buffer size {}, capacity {}", self.buffer.len(), self.capacity);
        while total_written < buf.len() {
            let mut fifo = self.buffer.lock();
            let capacity = self.capacity.lock();

            if fifo.len() == *capacity {
                drop(capacity);
                drop(fifo);
                
                // If buffer is full, wait for space
                self.write_waiter.lock().wait_current(Event::PipeWriteReady);
                current::schedule();
            } else {
                let to_write = core::cmp::min(buf.len() - total_written, *capacity - fifo.len());
                for i in 0..to_write {
                    fifo.push_back(buf[total_written + i]);
                }
                total_written += to_write;
                self.read_waiter.lock().wake_all(|e| e); // Wake up readers waiting for data
            }
        }
        
        Ok(total_written)
    }

    pub fn poll(&self, waker: usize, event: PollEventSet, writable: bool) -> SysResult<Option<PollEvent>> {
        if event.contains(PollEventSet::POLLIN) && writable {
            return Ok(None);
        }

        if event.contains(PollEventSet::POLLOUT) && !writable {
            return Ok(None);
        }
        
        let buffer = self.buffer.lock();
        if event.contains(PollEventSet::POLLIN) {
            if buffer.len() > 0 && !writable {
                return Ok(Some(PollEvent::ReadReady));
            } else {
                if *self.writer_count.lock() == 0 {
                    return Ok(Some(PollEvent::HangUp)); // No writers left, indicate EOF
                }
                self.read_waiter.lock().wait(current::tcb().clone(), Event::Poll{event: PollEvent::ReadReady, waker});
            }
        }

        if event.contains(PollEventSet::POLLOUT) {
            if buffer.len() < *self.capacity.lock() {
                return Ok(Some(PollEvent::WriteReady));
            } else {
                self.write_waiter.lock().wait(current::tcb().clone(), Event::Poll{event: PollEvent::WriteReady, waker});
            }
        }

        Ok(None)
    }

    pub fn poll_cancel(&self) {
        self.read_waiter.lock().remove(current::tcb());
        self.write_waiter.lock().remove(current::tcb());
    }

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
                    Event::Poll{ event: PollEvent::ReadReady, waker } => {Event::Poll{event: PollEvent::HangUp, waker} },
                    _ => e
                }
            }); // Wake up readers to notify them of EOF
        }
    }
}  