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

struct PtyState {
    open: PtyOpenState,
    master_recv: RingBuffer<u8, 4096>,
    master_waiters: WaitQueue<Event>,
    slave_waiters: WaitQueue<Event>,
}

pub(super) struct PtyInner {
    pub(super) id: usize,
    slave_name: String,
    pts_dir: Arc<dyn MemInodeOps<DevfsInfo>>,
    pub(super) tty: TtyState,
    state: SpinLock<PtyState>,
    master_open_count: AtomicUsize,
    slave_open_count: AtomicUsize,
    cleanup_done: AtomicBool,
    pub(super) master_epoll: Arc<EpollNotifier>,
    pub(super) slave_epoll: Arc<EpollNotifier>,
}

impl PtyInner {
    pub(super) fn new(id: usize, pts_dir: Arc<dyn MemInodeOps<DevfsInfo>>) -> Self {
        Self {
            id,
            slave_name: format!("{}", id),
            pts_dir,
            tty: TtyState::new(),
            state: SpinLock::new(
                PtyState {
                    open: PtyOpenState {
                        locked: true,
                        master_closed: false,
                        slave_closed: true,
                    },
                    master_recv: RingBuffer::new(0),
                    master_waiters: WaitQueue::new("PtyState::master_waiters"),
                    slave_waiters: WaitQueue::new("PtyState::slave_waiters"),
                },
                "PtyInner::state",
            ),
            master_open_count: AtomicUsize::new(0),
            slave_open_count: AtomicUsize::new(0),
            cleanup_done: AtomicBool::new(false),
            master_epoll: Arc::new(EpollNotifier::new()),
            slave_epoll: Arc::new(EpollNotifier::new()),
        }
    }

    pub(super) fn master_open(&self) {
        let mut state = self.state.lock();
        self.master_open_count.fetch_add(1, Ordering::Relaxed);
        state.open.master_closed = false;
    }

    pub(super) fn master_close(&self) {
        let previous = self.master_open_count.fetch_sub(1, Ordering::AcqRel);
        debug_assert!(previous > 0, "PtyInner::master_open_count underflow");
        if previous == 1 {
            self.hangup_master();
        }
    }

    pub(super) fn slave_open(&self) -> SysResult<()> {
        let mut state = self.state.lock();
        if state.open.locked || state.open.master_closed {
            return Err(Errno::EIO);
        }
        self.slave_open_count.fetch_add(1, Ordering::Relaxed);
        state.open.slave_closed = false;
        Ok(())
    }

    pub(super) fn slave_close(&self) {
        let previous = self.slave_open_count.fetch_sub(1, Ordering::AcqRel);
        debug_assert!(previous > 0, "PtyInner::slave_open_count underflow");
        if previous == 1 {
            let mut state = self.state.lock();
            state.open.slave_closed = true;
            self.wake_master_locked(&mut state, FileEvent::HANG_UP);
        }
    }

    pub(super) fn hangup_master(&self) {
        if self.cleanup_done.swap(true, Ordering::AcqRel) {
            return;
        }

        {
            let mut state = self.state.lock();
            state.open.master_closed = true;
            state.open.locked = true;
            self.wake_master_locked(&mut state, FileEvent::HANG_UP);
            self.wake_slave_locked(&mut state, FileEvent::HANG_UP);
        }

        let _ = self.pts_dir.as_ref().unlink(&self.slave_name);
    }

    pub(super) fn set_locked(&self, locked: bool) {
        self.state.lock().open.locked = locked;
    }

    pub(super) fn locked(&self) -> bool {
        self.state.lock().open.locked
    }

    pub fn flush_input(&self) {
        let _state = self.state.lock();
        self.tty.clear_input();
    }

    pub fn write_master(&self, buf: &[u8]) -> SysResult<usize> {
        let mut state = self.state.lock();
        if state.open.master_closed || state.open.slave_closed {
            return Err(Errno::EIO);
        }

        for &c in buf {
            let _ = self.tty.process_input_byte(c, |c| {
                state.master_recv.push(c);
            });
        }

        let slave_ready = self.tty.input_ready();
        let master_ready = !state.master_recv.empty();

        if slave_ready {
            self.wake_slave_locked(&mut state, FileEvent::READ_READY);
        }
        if master_ready {
            self.wake_master_locked(&mut state, FileEvent::READ_READY);
        }
        Ok(buf.len())
    }

    pub(super) fn write_slave(&self, buf: &[u8]) -> SysResult<usize> {
        let mut state = self.state.lock();
        if state.open.master_closed {
            return Err(Errno::EIO);
        }

        for &c in buf {
            self.tty.process_output_byte(c, |c| state.master_recv.push(c));
        }

        let ready = !state.master_recv.empty();
        if ready {
            self.wake_master_locked(&mut state, FileEvent::READ_READY);
        }
        Ok(buf.len())
    }

    pub fn read_master(&self, buf: &mut [u8], blocked: bool) -> SysResult<usize> {
        loop {
            {
                let mut state = self.state.lock();
                let mut drained = 0;
                for slot in buf.iter_mut() {
                    if let Some(c) = state.master_recv.pop() {
                        *slot = c;
                        drained += 1;
                    } else {
                        break;
                    }
                }
                if drained > 0 {
                    return Ok(drained);
                }
                if state.open.slave_closed || state.open.master_closed {
                    return Err(Errno::EIO);
                }
                if !blocked {
                    return Err(Errno::EAGAIN);
                }
                // Not ready, not hung up: register the waiter under the same
                // lock a waker must take, then drop the guard before scheduling.
                state.master_waiters.wait_current(Event::ReadReady);
            }

            current::schedule();
            match current::task().take_wakeup_event().unwrap() {
                Event::ReadReady => continue,
                Event::Signal => {
                    let mut state = self.state.lock();
                    state.master_waiters.remove_current();
                    return Err(Errno::EINTR);
                }
                _ => unreachable!(),
            }
        }
    }

    pub fn read_slave(&self, buf: &mut [u8], blocked: bool) -> SysResult<usize> {
        loop {
            {
                let mut state = self.state.lock();
                if let Some(n) = self.tty.read_input(buf) {
                    return Ok(n);
                }
                if state.open.master_closed {
                    return Ok(0);
                }
                if !blocked {
                    return Err(Errno::EAGAIN);
                }
                state.slave_waiters.wait_current(Event::ReadReady);
            }

            current::schedule();
            match current::task().take_wakeup_event().unwrap() {
                Event::ReadReady => continue,
                Event::Signal => {
                    let mut state = self.state.lock();
                    state.slave_waiters.remove_current();
                    return Err(Errno::EINTR);
                }
                _ => unreachable!(),
            }
        }
    }

    pub fn master_poll_event(&self, event: FileEvent) -> Option<FileEvent> {
        let state = self.state.lock();
        self.master_ready_locked(&state, event)
    }

    pub fn slave_poll_event(&self, event: FileEvent) -> Option<FileEvent> {
        let state = self.state.lock();
        self.slave_ready_locked(&state, event)
    }

    // -- poll-style wait helpers (used by file.rs) ---------------------------

    /// Called by `PtmxFile::wait_event` after a poll has returned `None`.
    /// Re-checks readiness under the lock (closing the race) and, if still
    /// not ready, registers the poll waiter before returning `None`.
    pub(super) fn master_wait_event(&self, waker: usize, event: FileEvent) -> SysResult<Option<FileEvent>> {
        let mut state = self.state.lock();
        if let Some(ready) = self.master_ready_locked(&state, event) {
            return Ok(Some(ready));
        }
        if event.contains(FileEvent::READ_READY) {
            state.master_waiters.wait_pending(
                current::task().clone(),
                Event::Poll {
                    event: FileEvent::READ_READY,
                    waker,
                },
            );
        }
        Ok(None)
    }

    pub(super) fn slave_wait_event(&self, waker: usize, event: FileEvent) -> SysResult<Option<FileEvent>> {
        let mut state = self.state.lock();
        if let Some(ready) = self.slave_ready_locked(&state, event) {
            return Ok(Some(ready));
        }
        if event.contains(FileEvent::READ_READY) {
            state.slave_waiters.wait_pending(
                current::task().clone(),
                Event::Poll {
                    event: FileEvent::READ_READY,
                    waker,
                },
            );
        }
        Ok(None)
    }

    pub(super) fn master_cancel_wait(&self) {
        let mut state = self.state.lock();
        state.master_waiters.remove_current();
    }

    pub(super) fn slave_cancel_wait(&self) {
        let mut state = self.state.lock();
        state.slave_waiters.remove_current();
    }

    // -- locked helpers (caller already holds `state`) -----------------------

    fn wake_master_locked(&self, state: &mut PtyState, event: FileEvent) {
        state.master_waiters.wake_all(|wait_event| match wait_event {
            Event::Poll { event: interest, waker } => Event::Poll {
                event: Self::poll_wake_event(event, interest),
                waker,
            },
            event => event,
        });
        self.master_epoll.notify(event);
    }

    fn wake_slave_locked(&self, state: &mut PtyState, event: FileEvent) {
        state.slave_waiters.wake_all(|wait_event| match wait_event {
            Event::Poll { event: interest, waker } => Event::Poll {
                event: Self::poll_wake_event(event, interest),
                waker,
            },
            event => event,
        });
        self.slave_epoll.notify(event);
    }

    fn poll_wake_event(event: FileEvent, interest: FileEvent) -> FileEvent {
        (event & interest) | (event & (FileEvent::ERROR | FileEvent::HANG_UP))
    }

    fn master_ready_locked(&self, state: &PtyState, event: FileEvent) -> Option<FileEvent> {
        let mut ready = FileEvent::empty();
        if event.contains(FileEvent::WRITE_READY) && !state.open.slave_closed && !state.open.master_closed {
            ready |= FileEvent::WRITE_READY;
        }
        if event.contains(FileEvent::READ_READY) && !state.master_recv.empty() {
            ready |= FileEvent::READ_READY;
        }
        if state.open.slave_closed || state.open.master_closed {
            ready |= FileEvent::HANG_UP;
        }
        if ready.is_empty() { None } else { Some(ready) }
    }

    fn slave_ready_locked(&self, state: &PtyState, event: FileEvent) -> Option<FileEvent> {
        let mut ready = FileEvent::empty();
        if event.contains(FileEvent::WRITE_READY) && !state.open.master_closed {
            ready |= FileEvent::WRITE_READY;
        }
        if event.contains(FileEvent::READ_READY) && self.tty.input_ready() {
            ready |= FileEvent::READ_READY;
        }
        if state.open.master_closed {
            ready |= FileEvent::HANG_UP;
        }
        if ready.is_empty() { None } else { Some(ready) }
    }
}
