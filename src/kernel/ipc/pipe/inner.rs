use alloc::collections::VecDeque;
use spin::Mutex;

use crate::kernel::errno::{Errno, SysResult};
use crate::kernel::event::{Event, FileEvent, PollEventSet, WaitQueue};
use crate::kernel::scheduler::current;

pub struct PipeInner {
    buffer: Mutex<VecDeque<u8>>,
    read_waiter: Mutex<WaitQueue<Event>>,
    write_waiter: Mutex<WaitQueue<Event>>,
    capacity: Mutex<usize>,
    // reader_count: Mutex<u32>,
    writer_count: Mutex<u32>,
}

impl PipeInner {
    pub fn new(capacity: usize) -> Self {
        Self {
            buffer: Mutex::new(VecDeque::with_capacity(capacity)),
            read_waiter: Mutex::new(WaitQueue::new()),
            write_waiter: Mutex::new(WaitQueue::new()),
            capacity: Mutex::new(capacity),
            // reader_count: Mutex::new(0),
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
        let cap = self.capacity.lock();

        if buf.len() >= *cap {
            let mut fifo = self.buffer.lock();
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
                fifo = self.buffer.lock();
                if *cap - fifo.len() >= buf.len() {
                    break;
                } else {
                    drop(cap);
                    drop(fifo);
                    // If buffer is full, wait for space
                    self.write_waiter.lock().wait_current(Event::PipeWriteReady);
                    current::schedule();

                    match current::task().take_wakeup_event().unwrap() {
                        Event::PipeWriteReady => {},
                        Event::Signal => return Err(Errno::EINTR),
                        _ => unreachable!()
                    }
                }
            }

            for i in 0..buf.len() {
                fifo.push_back(buf[i]);
            }

            self.read_waiter.lock().wake_all(|e| e); // Wake up readers waiting for data

            Ok(buf.len())
        }

        // while total_written < buf.len() {
        //     let mut fifo = self.buffer.lock();
        //     let capacity = self.capacity.lock();

        //     if fifo.len() == *capacity {
        //         drop(capacity);
        //         drop(fifo);
                
        //         // If buffer is full, wait for space
        //         self.write_waiter.lock().wait_current(Event::PipeWriteReady);
        //         current::schedule();
        //     } else {
        //         let to_write = core::cmp::min(buf.len() - total_written, *capacity - fifo.len());
        //         for i in 0..to_write {
        //             fifo.push_back(buf[total_written + i]);
        //         }
        //         total_written += to_write;
        //         self.read_waiter.lock().wake_all(|e| e); // Wake up readers waiting for data
        //     }
        // }
        
        // Ok(total_written)
    }

    pub fn wait_event(&self, waker: usize, event: PollEventSet, writable: bool) -> SysResult<Option<FileEvent>> {
        if event.contains(PollEventSet::POLLIN) && writable {
            return Ok(None);
        }

        if event.contains(PollEventSet::POLLOUT) && !writable {
            return Ok(None);
        }
        
        let buffer = self.buffer.lock();
        if event.contains(PollEventSet::POLLIN) {
            if buffer.len() > 0 && !writable {
                return Ok(Some(FileEvent::ReadReady));
            } else {
                if *self.writer_count.lock() == 0 {
                    return Ok(Some(FileEvent::HangUp)); // No writers left, indicate EOF
                }
                self.read_waiter.lock().wait(current::task().clone(), Event::Poll{event: FileEvent::ReadReady, waker});
            }
        }

        if event.contains(PollEventSet::POLLOUT) {
            if buffer.len() < *self.capacity.lock() {
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