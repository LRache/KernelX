use alloc::sync::Arc;

use crate::fs::file::{FileFlags, FileOps, SeekWhence};
use crate::fs::{Dentry, InodeOps, Mode};
use crate::kernel::config;
use crate::kernel::errno::{Errno, SysResult};
use crate::kernel::event::{FileEvent, PollEventSet};
use crate::kernel::ipc::pipe::PipeInner;
use crate::kernel::mm::ubuf::UAddrSpaceBuffer;
use crate::kernel::uapi::FileStat;
use crate::klib::SpinLock;

use super::msgpipe::MessagePipeInner;

/// The channel type determines how data flows through the socket.
enum Channel {
    /// Byte-stream (SOCK_STREAM): uses PipeInner, no message boundaries.
    Stream { rx: Arc<PipeInner>, tx: Arc<PipeInner> },
    /// Message-oriented (SOCK_DGRAM, SOCK_SEQPACKET): preserves message boundaries.
    Message {
        rx: Arc<MessagePipeInner>,
        tx: Arc<MessagePipeInner>,
    },
}

/// One endpoint of a Unix domain socket pair.
pub struct UnixSocket {
    channel: Channel,
    blocked: SpinLock<bool>,
    sock_type: SocketType,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum SocketType {
    Stream,
    Dgram,
    SeqPacket,
}

impl UnixSocket {
    /// Create a connected pair of Unix domain sockets.
    pub fn create_pair(sock_type: SocketType, blocked: bool) -> (Self, Self) {
        let capacity = config::PIPE_CAPACITY;

        match sock_type {
            SocketType::Stream => {
                let pipe_a = Arc::new(PipeInner::new(capacity));
                let pipe_b = Arc::new(PipeInner::new(capacity));

                pipe_a.increment_reader_count();
                pipe_a.increment_writer_count();
                pipe_b.increment_reader_count();
                pipe_b.increment_writer_count();

                let sock_a = UnixSocket {
                    channel: Channel::Stream {
                        rx: pipe_a.clone(),
                        tx: pipe_b.clone(),
                    },
                    blocked: SpinLock::new(blocked, "UnixSocket::blocked"),
                    sock_type,
                };

                let sock_b = UnixSocket {
                    channel: Channel::Stream { rx: pipe_b, tx: pipe_a },
                    blocked: SpinLock::new(blocked, "UnixSocket::blocked"),
                    sock_type,
                };

                (sock_a, sock_b)
            }
            SocketType::Dgram | SocketType::SeqPacket => {
                let pipe_a = Arc::new(MessagePipeInner::new(capacity));
                let pipe_b = Arc::new(MessagePipeInner::new(capacity));

                pipe_a.increment_reader_count();
                pipe_a.increment_writer_count();
                pipe_b.increment_reader_count();
                pipe_b.increment_writer_count();

                let sock_a = UnixSocket {
                    channel: Channel::Message {
                        rx: pipe_a.clone(),
                        tx: pipe_b.clone(),
                    },
                    blocked: SpinLock::new(blocked, "UnixSocket::blocked"),
                    sock_type,
                };

                let sock_b = UnixSocket {
                    channel: Channel::Message { rx: pipe_b, tx: pipe_a },
                    blocked: SpinLock::new(blocked, "UnixSocket::blocked"),
                    sock_type,
                };

                (sock_a, sock_b)
            }
        }
    }
}

impl FileOps for UnixSocket {
    fn read(&self, buf: &mut [u8]) -> SysResult<usize> {
        let blocked = *self.blocked.lock();
        match &self.channel {
            Channel::Stream { rx, .. } => rx.read(buf, blocked),
            Channel::Message { rx, .. } => rx.read(buf, blocked),
        }
    }

    fn pread(&self, _: &mut [u8], _: usize) -> SysResult<usize> {
        Err(Errno::ESPIPE)
    }

    fn read_to_user(&self, ubuf: &UAddrSpaceBuffer) -> SysResult<usize> {
        let blocked = *self.blocked.lock();
        match &self.channel {
            Channel::Stream { rx, .. } => rx.read_to_user(ubuf, blocked),
            Channel::Message { rx, .. } => rx.read_to_user(ubuf, blocked),
        }
    }

    fn write(&self, buf: &[u8]) -> SysResult<usize> {
        let blocked = *self.blocked.lock();
        match &self.channel {
            Channel::Stream { tx, .. } => tx.write(buf, blocked),
            Channel::Message { tx, .. } => tx.write(buf, blocked),
        }
    }

    fn pwrite(&self, _: &[u8], _: usize) -> SysResult<usize> {
        Err(Errno::ESPIPE)
    }

    fn write_from_user(&self, ubuf: &UAddrSpaceBuffer) -> SysResult<usize> {
        let blocked = *self.blocked.lock();
        match &self.channel {
            Channel::Stream { tx, .. } => tx.write_from_user(ubuf, blocked),
            Channel::Message { tx, .. } => tx.write_from_user(ubuf, blocked),
        }
    }

    fn readable(&self) -> bool {
        true
    }

    fn writable(&self) -> bool {
        true
    }

    fn block(&self) -> bool {
        *self.blocked.lock()
    }

    fn seek(&self, _: isize, _: SeekWhence) -> SysResult<usize> {
        Err(Errno::ESPIPE)
    }

    fn fstat(&self) -> SysResult<FileStat> {
        let mut kstat = FileStat::empty();
        kstat.st_mode = Mode::S_IFSOCK.bits() as u32 | 0o666;
        kstat.st_nlink = 1;
        Ok(kstat)
    }

    fn fsync(&self) -> SysResult<()> {
        Ok(())
    }

    fn get_inode(&self) -> Option<&Arc<dyn InodeOps>> {
        None
    }

    fn get_dentry(&self) -> Option<&Arc<Dentry>> {
        None
    }

    fn wait_event(&self, waker: usize, event: PollEventSet) -> SysResult<Option<FileEvent>> {
        match &self.channel {
            Channel::Stream { rx, tx, .. } => {
                if event.contains(PollEventSet::POLLIN) {
                    if let Some(ev) = rx.wait_event(waker, PollEventSet::POLLIN, false)? {
                        return Ok(Some(ev));
                    }
                }
                if event.contains(PollEventSet::POLLOUT) {
                    if let Some(ev) = tx.wait_event(waker, PollEventSet::POLLOUT, true)? {
                        return Ok(Some(ev));
                    }
                }
                Ok(None)
            }
            Channel::Message { rx, tx, .. } => {
                if event.contains(PollEventSet::POLLIN) {
                    if let Some(ev) = rx.wait_event(waker, PollEventSet::POLLIN, false)? {
                        return Ok(Some(ev));
                    }
                }
                if event.contains(PollEventSet::POLLOUT) {
                    if let Some(ev) = tx.wait_event(waker, PollEventSet::POLLOUT, true)? {
                        return Ok(Some(ev));
                    }
                }
                Ok(None)
            }
        }
    }

    fn wait_event_cancel(&self) {
        match &self.channel {
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

    fn set_flags(&self, flags: FileFlags) {
        *self.blocked.lock() = flags.blocked;
    }

    fn type_name(&self) -> &'static str {
        match self.sock_type {
            SocketType::Stream => "unix-stream",
            SocketType::Dgram => "unix-dgram",
            SocketType::SeqPacket => "unix-seqpacket",
        }
    }
}

impl Drop for UnixSocket {
    fn drop(&mut self) {
        match &self.channel {
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
