use alloc::string::String;
use alloc::sync::Arc;
use alloc::vec::Vec;

use crate::fs::file::{FileFlags, FileOps};
use crate::fs::{Dentry, InodeOps, Mode};
use crate::kernel::config;
use crate::kernel::errno::{Errno, SysResult};
use crate::kernel::event::{EpollNotifier, FileEvent};
use crate::kernel::ipc::pipe::PipeInner;
use crate::kernel::mm::ubuf::UAddrSpaceBuffer;
use crate::kernel::uapi::FileStat;
use crate::klib::SpinLock;

use super::msgpipe::MessagePipeInner;

/// The channel type determines how data flows through the socket.
#[derive(Clone)]
enum Channel {
    /// Freshly created via socket(), not yet connected to a peer.
    Unconnected,
    /// Byte-stream (SOCK_STREAM): uses PipeInner, no message boundaries.
    Stream { rx: Arc<PipeInner>, tx: Arc<PipeInner> },
    /// Message-oriented (SOCK_DGRAM, SOCK_SEQPACKET): preserves message boundaries.
    Message {
        rx: Arc<MessagePipeInner>,
        tx: Arc<MessagePipeInner>,
    },
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum UnixSocketType {
    Stream,
    Dgram,
    SeqPacket,
}

pub struct UnixSocket {
    channel: SpinLock<Channel>,
    blocked: SpinLock<bool>,
    bound_path: SpinLock<Option<String>>,
    sock_type: UnixSocketType,
}

impl UnixSocket {
    /// Create a fresh Unix domain socket for socket().
    pub fn new(sock_type: UnixSocketType, blocked: bool) -> Self {
        Self {
            channel: SpinLock::new(Channel::Unconnected, "UnixSocket::channel"),
            blocked: SpinLock::new(blocked, "UnixSocket::blocked"),
            bound_path: SpinLock::new(None, "UnixSocket::bound_path"),
            sock_type,
        }
    }

    /// Create a connected pair of Unix domain sockets.
    pub fn create_pair(sock_type: UnixSocketType, blocked: bool) -> (Self, Self) {
        let capacity = config::PIPE_CAPACITY;

        match sock_type {
            UnixSocketType::Stream => {
                let pipe_a = Arc::new(PipeInner::new(capacity));
                let pipe_b = Arc::new(PipeInner::new(capacity));

                pipe_a.increment_reader_count();
                pipe_a.increment_writer_count();
                pipe_b.increment_reader_count();
                pipe_b.increment_writer_count();

                let sock_a = UnixSocket {
                    channel: SpinLock::new(
                        Channel::Stream {
                            rx: pipe_a.clone(),
                            tx: pipe_b.clone(),
                        },
                        "UnixSocket::channel",
                    ),
                    blocked: SpinLock::new(blocked, "UnixSocket::blocked"),
                    bound_path: SpinLock::new(None, "UnixSocket::bound_path"),
                    sock_type,
                };

                let sock_b = UnixSocket {
                    channel: SpinLock::new(Channel::Stream { rx: pipe_b, tx: pipe_a }, "UnixSocket::channel"),
                    blocked: SpinLock::new(blocked, "UnixSocket::blocked"),
                    bound_path: SpinLock::new(None, "UnixSocket::bound_path"),
                    sock_type,
                };

                (sock_a, sock_b)
            }
            UnixSocketType::Dgram | UnixSocketType::SeqPacket => {
                let pipe_a = Arc::new(MessagePipeInner::new(capacity));
                let pipe_b = Arc::new(MessagePipeInner::new(capacity));

                pipe_a.increment_reader_count();
                pipe_a.increment_writer_count();
                pipe_b.increment_reader_count();
                pipe_b.increment_writer_count();

                let sock_a = UnixSocket {
                    channel: SpinLock::new(
                        Channel::Message {
                            rx: pipe_a.clone(),
                            tx: pipe_b.clone(),
                        },
                        "UnixSocket::channel",
                    ),
                    blocked: SpinLock::new(blocked, "UnixSocket::blocked"),
                    bound_path: SpinLock::new(None, "UnixSocket::bound_path"),
                    sock_type,
                };

                let sock_b = UnixSocket {
                    channel: SpinLock::new(Channel::Message { rx: pipe_b, tx: pipe_a }, "UnixSocket::channel"),
                    blocked: SpinLock::new(blocked, "UnixSocket::blocked"),
                    bound_path: SpinLock::new(None, "UnixSocket::bound_path"),
                    sock_type,
                };

                (sock_a, sock_b)
            }
        }
    }

    fn unconnected_read_error(&self) -> SysResult<usize> {
        Err(Errno::ENOTCONN)
    }

    fn unconnected_write_error(&self) -> SysResult<usize> {
        match self.sock_type {
            UnixSocketType::Dgram => Err(Errno::EDESTADDRREQ),
            UnixSocketType::Stream | UnixSocketType::SeqPacket => Err(Errno::ENOTCONN),
        }
    }

    pub fn can_bind(&self) -> SysResult<()> {
        if self.bound_path.lock().is_some() {
            return Err(Errno::EINVAL);
        }

        if matches!(*self.channel.lock(), Channel::Unconnected) {
            Ok(())
        } else {
            Err(Errno::EINVAL)
        }
    }

    pub fn bind_path(&self, path: String) -> SysResult<()> {
        let mut bound_path = self.bound_path.lock();
        if bound_path.is_some() {
            return Err(Errno::EINVAL);
        }

        if !matches!(*self.channel.lock(), Channel::Unconnected) {
            return Err(Errno::EINVAL);
        }

        *bound_path = Some(path);
        Ok(())
    }
}

impl FileOps for UnixSocket {
    fn read(&self, buf: &mut [u8]) -> SysResult<usize> {
        let blocked = *self.blocked.lock();
        let channel = self.channel.lock().clone();
        match channel {
            Channel::Unconnected => self.unconnected_read_error(),
            Channel::Stream { rx, .. } => rx.read(buf, blocked),
            Channel::Message { rx, .. } => rx.read(buf, blocked),
        }
    }

    fn read_to_user(&self, ubuf: &UAddrSpaceBuffer) -> SysResult<usize> {
        let blocked = *self.blocked.lock();
        let channel = self.channel.lock().clone();
        match channel {
            Channel::Unconnected => self.unconnected_read_error(),
            Channel::Stream { rx, .. } => rx.read_to_user(ubuf, blocked),
            Channel::Message { rx, .. } => rx.read_to_user(ubuf, blocked),
        }
    }

    fn write(&self, buf: &[u8]) -> SysResult<usize> {
        let blocked = *self.blocked.lock();
        let channel = self.channel.lock().clone();
        match channel {
            Channel::Unconnected => self.unconnected_write_error(),
            Channel::Stream { tx, .. } => tx.write(buf, blocked),
            Channel::Message { tx, .. } => tx.write(buf, blocked),
        }
    }

    fn write_from_user(&self, ubuf: &UAddrSpaceBuffer) -> SysResult<usize> {
        let blocked = *self.blocked.lock();
        let channel = self.channel.lock().clone();
        match channel {
            Channel::Unconnected => self.unconnected_write_error(),
            Channel::Stream { tx, .. } => tx.write_from_user(ubuf, blocked),
            Channel::Message { tx, .. } => tx.write_from_user(ubuf, blocked),
        }
    }

    fn flags(&self) -> FileFlags {
        FileFlags {
            readable: true,
            writable: true,
            blocked: *self.blocked.lock(),
            append: false,
            direct: false,
        }
    }

    fn fstat(&self) -> SysResult<FileStat> {
        let mut kstat = FileStat::empty();
        kstat.st_mode = Mode::S_IFSOCK.bits() as u32 | 0o666;
        kstat.st_nlink = 1;
        Ok(kstat)
    }

    fn fsync(&self) -> SysResult<()> {
        Err(Errno::EINVAL)
    }

    fn get_inode(&self) -> Option<&Arc<dyn InodeOps>> {
        None
    }

    fn get_dentry(&self) -> Option<&Arc<Dentry>> {
        None
    }

    fn poll_event(&self, event: FileEvent) -> SysResult<Option<FileEvent>> {
        let channel = self.channel.lock().clone();
        match channel {
            Channel::Unconnected => Ok(Some(FileEvent::HANG_UP)),
            Channel::Stream { rx, tx, .. } => {
                let mut ready = FileEvent::empty();
                if let Some(ev) = rx.poll_event(event, false)? {
                    ready |= ev;
                }
                if let Some(ev) = tx.poll_event(event, true)? {
                    ready |= ev;
                }

                if ready.is_empty() { Ok(None) } else { Ok(Some(ready)) }
            }
            Channel::Message { rx, tx, .. } => {
                let mut ready = FileEvent::empty();
                if let Some(ev) = rx.poll_event(event, false)? {
                    ready |= ev;
                }
                if let Some(ev) = tx.poll_event(event, true)? {
                    ready |= ev;
                }

                if ready.is_empty() { Ok(None) } else { Ok(Some(ready)) }
            }
        }
    }

    fn wait_event(&self, waker: usize, event: FileEvent) -> SysResult<Option<FileEvent>> {
        let channel = self.channel.lock().clone();
        match channel {
            Channel::Unconnected => Ok(Some(FileEvent::HANG_UP)),
            Channel::Stream { rx, tx, .. } => {
                let mut ready = FileEvent::empty();
                let mut waiting_on_rx = false;
                let mut waiting_on_tx = false;

                if event.contains(FileEvent::READ_READY) {
                    if let Some(ev) = rx.wait_event(waker, FileEvent::READ_READY, false)? {
                        ready |= ev;
                    } else {
                        waiting_on_rx = true;
                    }
                }
                if event.contains(FileEvent::WRITE_READY) {
                    if let Some(ev) = tx.wait_event(waker, FileEvent::WRITE_READY, true)? {
                        ready |= ev;
                    } else {
                        waiting_on_tx = true;
                    }
                }

                if !ready.is_empty() {
                    if waiting_on_rx {
                        rx.wait_event_cancel();
                    }
                    if waiting_on_tx {
                        tx.wait_event_cancel();
                    }
                    Ok(Some(ready))
                } else {
                    Ok(None)
                }
            }
            Channel::Message { rx, tx, .. } => {
                let mut ready = FileEvent::empty();
                let mut waiting_on_rx = false;
                let mut waiting_on_tx = false;

                if event.contains(FileEvent::READ_READY) {
                    if let Some(ev) = rx.wait_event(waker, FileEvent::READ_READY, false)? {
                        ready |= ev;
                    } else {
                        waiting_on_rx = true;
                    }
                }
                if event.contains(FileEvent::WRITE_READY) {
                    if let Some(ev) = tx.wait_event(waker, FileEvent::WRITE_READY, true)? {
                        ready |= ev;
                    } else {
                        waiting_on_tx = true;
                    }
                }

                if !ready.is_empty() {
                    if waiting_on_rx {
                        rx.wait_event_cancel();
                    }
                    if waiting_on_tx {
                        tx.wait_event_cancel();
                    }
                    Ok(Some(ready))
                } else {
                    Ok(None)
                }
            }
        }
    }

    fn wait_event_cancel(&self) {
        let channel = self.channel.lock().clone();
        match channel {
            Channel::Unconnected => {}
            Channel::Stream { rx, tx, .. } => {
                rx.wait_event_cancel();
                tx.wait_event_cancel();
            }
            Channel::Message { rx, tx, .. } => {
                rx.wait_event_cancel();
                tx.wait_event_cancel();
            }
        }
    }

    fn epoll_notifiers(&self) -> Option<Vec<Arc<EpollNotifier>>> {
        let channel = self.channel.lock().clone();
        match channel {
            Channel::Unconnected => None,
            Channel::Stream { rx, tx, .. } => {
                let mut notifiers = Vec::new();
                notifiers.push(rx.epoll_notifier(false));
                notifiers.push(tx.epoll_notifier(true));
                Some(notifiers)
            }
            Channel::Message { rx, tx, .. } => {
                let mut notifiers = Vec::new();
                notifiers.push(rx.epoll_notifier(false));
                notifiers.push(tx.epoll_notifier(true));
                Some(notifiers)
            }
        }
    }

    fn set_flags(&self, flags: FileFlags) {
        *self.blocked.lock() = flags.blocked;
    }

    fn type_name(&self) -> &'static str {
        match self.sock_type {
            UnixSocketType::Stream => "unix-stream",
            UnixSocketType::Dgram => "unix-dgram",
            UnixSocketType::SeqPacket => "unix-seqpacket",
        }
    }
}

impl Drop for UnixSocket {
    fn drop(&mut self) {
        match &*self.channel.lock() {
            Channel::Unconnected => {}
            Channel::Stream { rx, tx } => {
                rx.decrement_reader_count();
                tx.decrement_writer_count();
            }
            Channel::Message { rx, tx } => {
                rx.decrement_reader_count();
                tx.decrement_writer_count();
            }
        }
    }
}
