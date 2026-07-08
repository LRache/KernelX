use alloc::format;
use alloc::string::String;
use alloc::sync::Arc;
use core::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use crate::driver::char::tty::TtyState;
use crate::fs::devfs::superblock::DevfsInfo;
use crate::fs::memtreefs::inode::MemInodeOps;
use crate::kernel::errno::{Errno, SysResult};
use crate::kernel::event::{EpollNotifier, Event, FileEvent, WaitQueue};
use crate::kernel::scheduler::current;
use crate::klib::SpinLock;
use crate::klib::ring::RingBuffer;

struct PtyOpenState {
    locked: bool,
    master_closed: bool,
    slave_closed: bool,
}

pub(super) struct PtyInner {
    pub(super) id: usize,
    slave_name: String,
    pts_dir: Arc<dyn MemInodeOps<DevfsInfo>>,
    pub(super) tty: TtyState,
    master_recv: SpinLock<RingBuffer<u8, 4096>>,
    pub(super) master_waiters: SpinLock<WaitQueue<Event>>,
    pub(super) slave_waiters: SpinLock<WaitQueue<Event>>,
    pub(super) master_epoll: Arc<EpollNotifier>,
    pub(super) slave_epoll: Arc<EpollNotifier>,
    state: SpinLock<PtyOpenState>,
    master_open_count: AtomicUsize,
    slave_open_count: AtomicUsize,
    cleanup_done: AtomicBool,
}

impl PtyInner {
    pub(super) fn new(id: usize, pts_dir: Arc<dyn MemInodeOps<DevfsInfo>>) -> Self {
        Self {
            id,
            slave_name: format!("{}", id),
            pts_dir,
            tty: TtyState::new(),
            master_recv: SpinLock::new(RingBuffer::new(0), "PtyInner::master_recv"),
            master_waiters: SpinLock::new(WaitQueue::new(), "PtyInner::master_waiters"),
            slave_waiters: SpinLock::new(WaitQueue::new(), "PtyInner::slave_waiters"),
            master_epoll: Arc::new(EpollNotifier::new()),
            slave_epoll: Arc::new(EpollNotifier::new()),
            state: SpinLock::new(
                PtyOpenState {
                    locked: true,
                    master_closed: false,
                    slave_closed: true,
                },
                "PtyInner::state",
            ),
            master_open_count: AtomicUsize::new(0),
            slave_open_count: AtomicUsize::new(0),
            cleanup_done: AtomicBool::new(false),
        }
    }

    pub(super) fn master_open(&self) {
        self.master_open_count.fetch_add(1, Ordering::Relaxed);
        self.state.lock().master_closed = false;
    }

    pub(super) fn master_close(&self) {
        let previous = self.master_open_count.fetch_sub(1, Ordering::AcqRel);
        debug_assert!(previous > 0, "PtyInner::master_open_count underflow");
        if previous == 1 {
            self.hangup_master();
        }
    }

    pub(super) fn slave_open(&self) -> SysResult<()> {
        {
            let mut state = self.state.lock();
            if state.locked || state.master_closed {
                return Err(Errno::EIO);
            }
            state.slave_closed = false;
        }
        self.slave_open_count.fetch_add(1, Ordering::Relaxed);
        Ok(())
    }

    pub(super) fn slave_close(&self) {
        let previous = self.slave_open_count.fetch_sub(1, Ordering::AcqRel);
        debug_assert!(previous > 0, "PtyInner::slave_open_count underflow");
        if previous == 1 {
            self.state.lock().slave_closed = true;
            self.wake_master(FileEvent::HANG_UP);
        }
    }

    pub(super) fn hangup_master(&self) {
        if self.cleanup_done.swap(true, Ordering::AcqRel) {
            return;
        }

        {
            let mut state = self.state.lock();
            state.master_closed = true;
            state.locked = true;
        }

        let _ = self.pts_dir.as_ref().unlink(&self.slave_name);
        self.wake_master(FileEvent::HANG_UP);
        self.wake_slave(FileEvent::HANG_UP);
    }

    pub(super) fn set_locked(&self, locked: bool) {
        self.state.lock().locked = locked;
    }

    pub(super) fn locked(&self) -> bool {
        self.state.lock().locked
    }

    fn slave_closed(&self) -> bool {
        self.state.lock().slave_closed
    }

    fn master_closed(&self) -> bool {
        self.state.lock().master_closed
    }

    fn wake_master(&self, event: FileEvent) {
        self.master_waiters.lock().wake_all(|e| e);
        self.master_epoll.notify(event);
    }

    fn wake_slave(&self, event: FileEvent) {
        self.slave_waiters.lock().wake_all(|e| e);
        self.slave_epoll.notify(event);
    }

    pub fn flush_input(&self) {
        self.tty.clear_input();
    }

    pub fn write_master(&self, buf: &[u8]) -> SysResult<usize> {
        if self.master_closed() || self.slave_closed() {
            return Err(Errno::EIO);
        }

        let mut echo = self.master_recv.lock();

        for &c in buf {
            let _ = self.tty.process_input_byte(c, |c| {
                echo.push(c);
            });
        }

        let slave_ready = self.tty.input_ready();
        let master_ready = !echo.empty();
        drop(echo);

        if slave_ready {
            self.wake_slave(FileEvent::READ_READY);
        }
        if master_ready {
            self.wake_master(FileEvent::READ_READY);
        }
        Ok(buf.len())
    }

    pub(super) fn write_slave(&self, buf: &[u8]) -> SysResult<usize> {
        if self.master_closed() {
            return Err(Errno::EIO);
        }

        let mut master_recv = self.master_recv.lock();
        for &c in buf {
            self.tty.process_output_byte(c, |c| master_recv.push(c));
        }
        let ready = !master_recv.empty();
        drop(master_recv);

        if ready {
            self.wake_master(FileEvent::READ_READY);
        }
        Ok(buf.len())
    }

    pub fn read_master(&self, buf: &mut [u8], blocked: bool) -> SysResult<usize> {
        self.read_buffer(
            buf,
            blocked,
            &self.master_recv,
            &self.master_waiters,
            "read_pty_master",
            || self.slave_closed(),
            Err(Errno::EIO),
        )
    }

    pub fn read_slave(&self, buf: &mut [u8], blocked: bool) -> SysResult<usize> {
        loop {
            if let Some(read) = self.tty.read_input(buf) {
                return Ok(read);
            }
            if self.master_closed() {
                return Ok(0);
            }
            if !blocked {
                return Err(Errno::EAGAIN);
            }

            self.slave_waiters.lock().wait_current(Event::ReadReady);
            current::schedule();
            match current::task().take_wakeup_event().unwrap() {
                Event::ReadReady => {}
                Event::Signal => {
                    self.slave_waiters.lock().remove_current();
                    return Err(Errno::EINTR);
                }
                _ => unreachable!(),
            }
        }
    }

    fn read_buffer<const N: usize>(
        &self,
        buf: &mut [u8],
        blocked: bool,
        ring: &SpinLock<RingBuffer<u8, N>>,
        waiters: &SpinLock<WaitQueue<Event>>,
        _block_reason: &'static str,
        hung_up: impl Fn() -> bool,
        hangup_result: SysResult<usize>,
    ) -> SysResult<usize> {
        loop {
            let mut read = 0;
            {
                let mut ring = ring.lock();
                for i in buf.iter_mut() {
                    if let Some(c) = ring.pop() {
                        *i = c;
                        read += 1;
                    } else {
                        break;
                    }
                }
            }

            if read > 0 {
                return Ok(read);
            }
            if hung_up() {
                return hangup_result;
            }
            if !blocked {
                return Err(Errno::EAGAIN);
            }

            waiters.lock().wait_current(Event::ReadReady);
            current::schedule();
            match current::task().take_wakeup_event().unwrap() {
                Event::ReadReady => {}
                Event::Signal => {
                    waiters.lock().remove_current();
                    return Err(Errno::EINTR);
                }
                _ => unreachable!(),
            }
        }
    }

    pub fn master_poll_event(&self, event: FileEvent) -> Option<FileEvent> {
        let mut ready = FileEvent::empty();
        if event.contains(FileEvent::WRITE_READY) && !self.slave_closed() && !self.master_closed() {
            ready |= FileEvent::WRITE_READY;
        }
        if event.contains(FileEvent::READ_READY) && !self.master_recv.lock().empty() {
            ready |= FileEvent::READ_READY;
        }
        if self.slave_closed() || self.master_closed() {
            ready |= FileEvent::HANG_UP;
        }
        if ready.is_empty() { None } else { Some(ready) }
    }

    pub fn slave_poll_event(&self, event: FileEvent) -> Option<FileEvent> {
        let mut ready = FileEvent::empty();
        if event.contains(FileEvent::WRITE_READY) && !self.master_closed() {
            ready |= FileEvent::WRITE_READY;
        }
        if event.contains(FileEvent::READ_READY) && self.tty.input_ready() {
            ready |= FileEvent::READ_READY;
        }
        if self.master_closed() {
            ready |= FileEvent::HANG_UP;
        }
        if ready.is_empty() { None } else { Some(ready) }
    }
}
