use alloc::sync::Arc;
use alloc::vec;
use alloc::vec::Vec;
use core::time::Duration;
use num_enum::TryFromPrimitive;

use crate::fs::file::FileOps;
use crate::fs::{Mode, Owner, vfs};
use crate::kernel::errno::{Errno, SysResult};
use crate::kernel::ipc::unixsocket::{UnixSocket, UnixSocketType};
use crate::kernel::scheduler::current;
use crate::kernel::scheduler::current::{copy_from_user, copy_to_user};
use crate::kernel::syscall::uptr::UArray;
use crate::kernel::task::CapabilitySet;
use crate::kernel::task::fdtable::FDFlags;
use crate::net::protocol::ipv4::IpProtocol;
use crate::net::socket::{
    AddressFamily, InetSocket, NetlinkProtocol, NetlinkSocket, SOCK_CLOEXEC, SOCK_NONBLOCK, SockAddrIn, SocketAddr,
};

use super::SyscallRet;
use super::common::IOVec;
use super::uptr::{UBuffer, UPtr, UserPointer, UserStruct};

const MSG_IOV_MAX: usize = 1024;
const MSG_DONTWAIT: usize = 0x40;
const SOL_SOCKET: usize = 1;
const SO_RCVTIMEO: usize = 20;
const SO_RCVTIMEO_NEW: usize = 66;
const UNIX_PATH_MAX: usize = 108;

#[repr(C)]
#[derive(Clone, Copy)]
struct SockTimeval {
    tv_sec: i64,
    tv_usec: i64,
}

impl UserStruct for SockTimeval {}

impl TryFrom<SockTimeval> for Option<Duration> {
    type Error = Errno;

    fn try_from(value: SockTimeval) -> SysResult<Self> {
        if value.tv_sec < 0 || value.tv_usec < 0 || value.tv_usec >= 1_000_000 {
            return Err(Errno::EINVAL);
        }
        if value.tv_sec == 0 && value.tv_usec == 0 {
            return Ok(None);
        }
        Ok(Some(Duration::new(value.tv_sec as u64, (value.tv_usec * 1000) as u32)))
    }
}

#[repr(C)]
#[derive(Clone, Copy)]
struct SocketAddrUn {
    sun_family: u16,
    sun_path: [u8; UNIX_PATH_MAX],
}

impl SocketAddrUn {
    fn bind_path(&self, addrlen: usize) -> SysResult<&str> {
        if addrlen < size_of::<u16>() || addrlen > size_of::<Self>() {
            return Err(Errno::EINVAL);
        }
        if self.sun_family != AddressFamily::Unix as u16 {
            return Err(Errno::EAFNOSUPPORT);
        }

        let mut path = &self.sun_path[..addrlen - size_of::<u16>()];
        if path.is_empty() {
            return Err(Errno::EINVAL);
        }
        if path[0] == 0 {
            return Err(Errno::EOPNOTSUPP);
        }

        if let Some(nul) = path.iter().position(|&byte| byte == 0) {
            path = &path[..nul];
        }
        if path.is_empty() {
            return Err(Errno::EINVAL);
        }

        core::str::from_utf8(path).map_err(|_| Errno::EINVAL)
    }
}

impl UserStruct for SocketAddrUn {}

#[repr(usize)]
#[derive(Clone, Copy, Debug, Eq, PartialEq, TryFromPrimitive)]
enum SocketKind {
    Stream = 1,
    Dgram = 2,
    Raw = 3,
    SeqPacket = 5,
}

fn unix_socket_type(sock_kind: SocketKind, protocol: usize) -> SysResult<UnixSocketType> {
    if protocol != 0 {
        return Err(Errno::EPROTONOSUPPORT);
    }

    match sock_kind {
        SocketKind::Stream => Ok(UnixSocketType::Stream),
        SocketKind::Dgram => Ok(UnixSocketType::Dgram),
        SocketKind::SeqPacket => Ok(UnixSocketType::SeqPacket),
        SocketKind::Raw => Err(Errno::EPROTONOSUPPORT),
    }
}

fn inet_socketpair_error(sock_kind: SocketKind, protocol: usize) -> Errno {
    match sock_kind {
        SocketKind::Dgram => match protocol {
            0 => Errno::EOPNOTSUPP,
            protocol if protocol == IpProtocol::Udp as usize => Errno::EOPNOTSUPP,
            _ => Errno::EPROTONOSUPPORT,
        },
        SocketKind::Stream => match protocol {
            0 => Errno::EOPNOTSUPP,
            protocol if protocol == IpProtocol::Tcp as usize => Errno::EOPNOTSUPP,
            _ => Errno::EPROTONOSUPPORT,
        },
        SocketKind::Raw => Errno::EPROTONOSUPPORT,
        SocketKind::SeqPacket => Errno::EINVAL,
    }
}

pub fn socketpair(domain: usize, sock_type: usize, protocol: usize, uptr_sv: UArray<i32>) -> SyscallRet {
    let flags = sock_type & (SOCK_NONBLOCK | SOCK_CLOEXEC);
    let base_type = sock_type & !(SOCK_NONBLOCK | SOCK_CLOEXEC);
    let domain = AddressFamily::try_from(domain).map_err(|_| Errno::EAFNOSUPPORT)?;
    let sock_kind = SocketKind::try_from(base_type).map_err(|_| Errno::EINVAL)?;

    let socket_type = match domain {
        AddressFamily::Unix => unix_socket_type(sock_kind, protocol)?,
        AddressFamily::Inet => return Err(inet_socketpair_error(sock_kind, protocol)),
        _ => return Err(Errno::EAFNOSUPPORT),
    };

    let blocked = flags & SOCK_NONBLOCK == 0;
    let cloexec = flags & SOCK_CLOEXEC != 0;
    let fd_flags = FDFlags { cloexec };

    let (sock_a, sock_b) = UnixSocket::create_pair(socket_type, blocked);
    let sock_a = Arc::new(sock_a);
    let sock_b = Arc::new(sock_b);

    let (fd_a, fd_b);
    {
        let fdtable = current::fdtable();
        let mut fdtable = fdtable.lock();
        fd_a = fdtable.push(sock_a, fd_flags)?;
        fd_b = match fdtable.push(sock_b, fd_flags) {
            Ok(fd) => fd,
            Err(err) => {
                let _ = fdtable.take(fd_a);
                return Err(err);
            }
        };
    }

    if let Err(err) = uptr_sv.write(0, &[fd_a as i32, fd_b as i32]) {
        let fdtable = current::fdtable();
        let mut fdtable = fdtable.lock();
        let _ = fdtable.take(fd_a);
        let _ = fdtable.take(fd_b);
        return Err(err);
    }

    Ok(0)
}

pub fn socket(domain: usize, sock_type: usize, protocol: usize) -> SyscallRet {
    let base_type = sock_type & !(SOCK_NONBLOCK | SOCK_CLOEXEC);
    let blocked = sock_type & SOCK_NONBLOCK == 0;
    let cloexec = sock_type & SOCK_CLOEXEC != 0;
    let domain = AddressFamily::try_from(domain).map_err(|_| Errno::EAFNOSUPPORT)?;
    let sock_kind = SocketKind::try_from(base_type).map_err(|_| Errno::EINVAL)?;

    let sock: Arc<dyn FileOps> = match domain {
        AddressFamily::Unix => Arc::new(UnixSocket::new(unix_socket_type(sock_kind, protocol)?, blocked)),
        AddressFamily::Inet => match sock_kind {
            SocketKind::Dgram => {
                if protocol != 0 && protocol != IpProtocol::Udp as usize {
                    return Err(Errno::EPROTONOSUPPORT);
                }
                Arc::new(InetSocket::new_udp(blocked))
            }
            SocketKind::Stream => {
                if protocol != 0 && protocol != IpProtocol::Tcp as usize {
                    return Err(Errno::EPROTONOSUPPORT);
                }
                Arc::new(InetSocket::new_tcp(blocked))
            }
            SocketKind::Raw => {
                if !current::capable(CapabilitySet::NET_RAW) {
                    return Err(Errno::EPERM);
                }
                let proto = u8::try_from(protocol).map_err(|_| Errno::EPROTONOSUPPORT)?;
                if proto == 0 {
                    return Err(Errno::EPROTONOSUPPORT);
                }
                Arc::new(InetSocket::new_raw(proto, blocked))
            }
            _ => return Err(Errno::EINVAL),
        },
        AddressFamily::Netlink => match sock_kind {
            SocketKind::Raw | SocketKind::Dgram => {
                NetlinkProtocol::try_from(protocol).map_err(|_| Errno::EPROTONOSUPPORT)?;
                Arc::new(NetlinkSocket::new(blocked))
            }
            _ => return Err(Errno::EINVAL),
        },
    };

    let fd = current::fdtable().lock().push(sock, FDFlags { cloexec })?;
    Ok(fd)
}

fn inet_socket(file: &Arc<dyn FileOps>) -> SysResult<&InetSocket> {
    file.downcast_ref::<InetSocket>().ok_or(Errno::ENOTSOCK)
}

fn is_netlink_socket(file: &Arc<dyn FileOps>) -> bool {
    file.downcast_ref::<NetlinkSocket>().is_some()
}

fn read_inet_sockaddr(addr_ptr: UPtr<SockAddrIn>, addrlen: usize) -> SysResult<SocketAddr> {
    if addrlen < size_of::<SockAddrIn>() {
        return Err(Errno::EINVAL);
    }

    let raw = addr_ptr.read()?;
    SocketAddr::from_raw(&raw)
}

fn read_sockaddr_un(addr_ptr: UPtr<u8>, addrlen: usize) -> SysResult<SocketAddrUn> {
    if addrlen < size_of::<u16>() || addrlen > size_of::<SocketAddrUn>() {
        return Err(Errno::EINVAL);
    }

    let mut family = [0u8; size_of::<u16>()];
    copy_from_user::buffer(addr_ptr.uaddr(), &mut family)?;

    let path_len = addrlen - size_of::<u16>();
    let mut sun_path = [0u8; UNIX_PATH_MAX];
    if path_len > 0 {
        copy_from_user::buffer(addr_ptr.uaddr() + size_of::<u16>(), &mut sun_path[..path_len])?;
    }

    Ok(SocketAddrUn {
        sun_family: u16::from_ne_bytes(family),
        sun_path,
    })
}

fn bind_unix_socket(sock: &Arc<UnixSocket>, addr_ptr: UPtr<u8>, addrlen: usize) -> SyscallRet {
    let sockaddr = read_sockaddr_un(addr_ptr, addrlen)?;
    let path = sockaddr.bind_path(addrlen)?;

    sock.can_bind()?;

    let (parent, name, absolute_path) = current::with_root_cwd(|root, cwd| {
        let (parent, name) = vfs::load_parent_dentry_at(&root, &cwd, path)?.ok_or(Errno::EINVAL)?;
        if name.as_ref() == "/" || name.is_empty() {
            return Err(Errno::EINVAL);
        }

        let name = name.into_owned();
        let mut absolute = parent.get_path();
        if !absolute.ends_with('/') {
            absolute.push('/');
        }
        absolute.push_str(&name);

        Ok((parent, name, absolute))
    })?;

    let mode = Mode::S_IFSOCK | Mode::from_bits_truncate(0o777 & !current::umask());
    let owner = Owner::new(current::fsuid(), current::fsgid());

    match parent.create(&name, mode, owner) {
        Ok(_) => {}
        Err(Errno::EEXIST) => return Err(Errno::EADDRINUSE),
        Err(err) => return Err(err),
    }

    if let Err(err) = sock.bind_path(absolute_path) {
        let _ = parent.unlink(&name);
        return Err(err);
    }

    Ok(0)
}

pub fn bind(fd: usize, addr_ptr: UPtr<u8>, addrlen: usize) -> SyscallRet {
    let file = current::fdtable().lock().get(fd)?;
    // Netlink sockets: bind is a no-op (kernel assigns pid automatically)
    if is_netlink_socket(&file) {
        return Ok(0);
    }
    if let Ok(sock) = file.clone().downcast_arc::<UnixSocket>() {
        return bind_unix_socket(&sock, addr_ptr, addrlen);
    }

    let addr = read_inet_sockaddr(UPtr::<SockAddrIn>::from_uaddr(addr_ptr.uaddr()), addrlen)?;
    inet_socket(&file)?.bind(addr)?;
    Ok(0)
}

pub fn listen(fd: usize, backlog: usize) -> SyscallRet {
    let file = current::fdtable().lock().get(fd)?;
    inet_socket(&file)?.listen(backlog)?;
    Ok(0)
}

fn write_inet_sockaddr(addr_ptr: UPtr<SockAddrIn>, addrlen_ptr: UPtr<u32>, addr: SocketAddr) -> SysResult<()> {
    if addr_ptr.is_null() || addrlen_ptr.is_null() {
        return Err(Errno::EFAULT);
    }

    let user_len = addrlen_ptr.read()? as usize;
    let sockaddr_len = core::mem::size_of::<SockAddrIn>();
    if user_len < sockaddr_len {
        return Err(Errno::EINVAL);
    }

    addr_ptr.write(addr.to_raw())?;
    addrlen_ptr.write(sockaddr_len as u32)?;
    Ok(())
}

fn accept_with_flags(fd: usize, addr_ptr: UPtr<SockAddrIn>, addrlen_ptr: UPtr<u32>, flags: usize) -> SyscallRet {
    if flags & !(SOCK_NONBLOCK | SOCK_CLOEXEC) != 0 {
        return Err(Errno::EINVAL);
    }

    let file = current::fdtable().lock().get(fd)?;
    let new_sock = inet_socket(&file)?.accept()?;
    if flags & SOCK_NONBLOCK != 0 {
        let mut file_flags = new_sock.flags();
        file_flags.blocked = false;
        new_sock.set_flags(file_flags);
    }

    // Write peer address to user if requested
    if !addr_ptr.is_null() {
        let peer = new_sock.peer_addr().ok_or(Errno::ENOTCONN)?;
        write_inet_sockaddr(addr_ptr, addrlen_ptr, peer)?;
    }

    let new_fd = current::fdtable().lock().push(
        new_sock as Arc<dyn FileOps>,
        FDFlags {
            cloexec: flags & SOCK_CLOEXEC != 0,
        },
    )?;
    Ok(new_fd)
}

pub fn accept(fd: usize, addr_ptr: UPtr<SockAddrIn>, addrlen_ptr: UPtr<u32>) -> SyscallRet {
    accept_with_flags(fd, addr_ptr, addrlen_ptr, 0)
}

pub fn accept4(fd: usize, addr_ptr: UPtr<SockAddrIn>, addrlen_ptr: UPtr<u32>, flags: usize) -> SyscallRet {
    accept_with_flags(fd, addr_ptr, addrlen_ptr, flags)
}

pub fn connect(fd: usize, addr_ptr: UPtr<SockAddrIn>, addrlen: usize) -> SyscallRet {
    let file = current::fdtable().lock().get(fd)?;
    let addr = read_inet_sockaddr(addr_ptr, addrlen)?;
    inet_socket(&file)?.connect(addr)?;
    Ok(0)
}

pub fn getsockname(fd: usize, addr_ptr: UPtr<SockAddrIn>, addrlen_ptr: UPtr<u32>) -> SyscallRet {
    let file = current::fdtable().lock().get(fd)?;
    if is_netlink_socket(&file) {
        return Err(Errno::EOPNOTSUPP);
    }

    let addr = inet_socket(&file)?.local_addr().unwrap_or(SocketAddr::any(0));
    write_inet_sockaddr(addr_ptr, addrlen_ptr, addr)?;
    Ok(0)
}

pub fn getpeername(fd: usize, addr_ptr: UPtr<SockAddrIn>, addrlen_ptr: UPtr<u32>) -> SyscallRet {
    let file = current::fdtable().lock().get(fd)?;
    if is_netlink_socket(&file) {
        return Err(Errno::EOPNOTSUPP);
    }

    let addr = inet_socket(&file)?.peer_addr().ok_or(Errno::ENOTCONN)?;
    write_inet_sockaddr(addr_ptr, addrlen_ptr, addr)?;
    Ok(0)
}

fn read_optional_inet_sockaddr(addr_ptr: UPtr<SockAddrIn>, addrlen: usize) -> SysResult<Option<SocketAddr>> {
    if addr_ptr.is_null() {
        return Ok(None);
    }
    read_inet_sockaddr(addr_ptr, addrlen).map(Some)
}

fn allow_block(flags: usize) -> bool {
    flags & MSG_DONTWAIT == 0
}

fn send_socket(file: &Arc<dyn FileOps>, buf: &[u8], dst: Option<SocketAddr>, flags: usize) -> SyscallRet {
    if is_netlink_socket(file) {
        return file.write(buf);
    }

    inet_socket(file)?.sendto_with_blocking(buf, dst, allow_block(flags))
}

pub fn sendto(
    fd: usize,
    buf_ptr: UBuffer,
    len: usize,
    flags: usize,
    addr_ptr: UPtr<SockAddrIn>,
    addrlen: usize,
) -> SyscallRet {
    let file = current::fdtable().lock().get(fd)?;

    let mut kbuf = alloc::vec![0u8; len];
    buf_ptr.read(0, &mut kbuf)?;

    let dst = read_optional_inet_sockaddr(addr_ptr, addrlen)?;
    send_socket(&file, &kbuf, dst, flags)
}

fn recv_socket(file: &Arc<dyn FileOps>, buf: &mut [u8], flags: usize) -> SysResult<(usize, Option<SocketAddr>)> {
    if is_netlink_socket(file) {
        return Ok((file.read(buf)?, None));
    }

    inet_socket(file)?.recvfrom_with_blocking(buf, allow_block(flags))
}

pub fn recvfrom(
    fd: usize,
    buf_ptr: UBuffer,
    len: usize,
    flags: usize,
    addr_ptr: UPtr<SockAddrIn>,
    addrlen_ptr: UPtr<u32>,
) -> SyscallRet {
    let file = current::fdtable().lock().get(fd)?;

    let mut kbuf = alloc::vec![0u8; len];
    let (n, src) = recv_socket(&file, &mut kbuf, flags)?;

    // Write data back to user
    buf_ptr.write(0, &kbuf[..n])?;

    // Write source address if requested
    if !addr_ptr.is_null() {
        if let Some(src) = src {
            let raw = src.to_raw();
            addr_ptr.write(raw)?;
        }
        if !addrlen_ptr.is_null() {
            addrlen_ptr.write(core::mem::size_of::<SockAddrIn>() as u32)?;
        }
    }

    Ok(n)
}

fn read_sockopt_int(optval: usize, optlen: usize) -> SysResult<usize> {
    if optval == 0 {
        return Err(Errno::EFAULT);
    }
    if optlen < size_of::<i32>() {
        return Err(Errno::EINVAL);
    }

    let value = UPtr::<i32>::from_uaddr(optval).read()?;
    if value < 0 {
        return Err(Errno::EINVAL);
    }
    Ok(value as usize)
}

fn read_sockopt_timeval(optval: usize, optlen: usize) -> SysResult<Option<Duration>> {
    if optval == 0 {
        return Err(Errno::EFAULT);
    }
    if optlen < size_of::<SockTimeval>() {
        return Err(Errno::EINVAL);
    }

    UPtr::<SockTimeval>::from_uaddr(optval).read()?.try_into()
}

pub fn setsockopt(fd: usize, level: usize, optname: usize, optval: usize, optlen: usize) -> SyscallRet {
    let file = current::fdtable().lock().get(fd)?;
    if is_netlink_socket(&file) {
        return Ok(0);
    }
    if level == SOL_SOCKET && matches!(optname, SO_RCVTIMEO | SO_RCVTIMEO_NEW) {
        let timeout = read_sockopt_timeval(optval, optlen)?;
        inet_socket(&file)?.set_recv_timeout(timeout);
        return Ok(0);
    }

    let value = read_sockopt_int(optval, optlen)?;
    inet_socket(&file)?.setsockopt(level, optname, value)?;
    Ok(0)
}

pub fn getsockopt(fd: usize, level: usize, optname: usize, optval: usize, optlen: usize) -> SyscallRet {
    let file = current::fdtable().lock().get(fd)?;
    if is_netlink_socket(&file) {
        return Ok(0);
    }

    if optval == 0 || optlen == 0 {
        return Err(Errno::EFAULT);
    }
    let optlen_ptr = UPtr::<u32>::from_uaddr(optlen);
    let user_len = optlen_ptr.read()? as usize;
    if user_len < size_of::<i32>() {
        return Err(Errno::EINVAL);
    }

    let value = inet_socket(&file)?.getsockopt(level, optname)?.min(i32::MAX as usize) as i32;
    UPtr::<i32>::from_uaddr(optval).write(value)?;
    optlen_ptr.write(size_of::<i32>() as u32)?;
    Ok(0)
}

pub fn shutdown(fd: usize, how: usize) -> SyscallRet {
    let file = current::fdtable().lock().get(fd)?;
    if is_netlink_socket(&file) {
        return Ok(0);
    }
    inet_socket(&file)?.shutdown(how)?;
    Ok(0)
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct UMsgHdr {
    msg_name: usize,
    msg_namelen: u32,
    msg_iov: usize,
    msg_iovlen: usize,
    msg_control: usize,
    msg_controllen: usize,
    msg_flags: i32,
}

impl UserStruct for UMsgHdr {}

impl UMsgHdr {
    fn validate(&self) -> SysResult<()> {
        if self.msg_iovlen > MSG_IOV_MAX {
            return Err(Errno::EINVAL);
        }
        if self.msg_iovlen > 0 && self.msg_iov == 0 {
            return Err(Errno::EFAULT);
        }
        if self.msg_control != 0 || self.msg_controllen != 0 {
            return Err(Errno::EOPNOTSUPP);
        }
        Ok(())
    }
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct UMMsgHdr {
    msg_hdr: UMsgHdr,
    msg_len: u32,
}

impl UserStruct for UMMsgHdr {}

fn total_iov_len(iov_base: usize, iovcnt: usize) -> Result<usize, Errno> {
    if iovcnt == 0 {
        return Ok(0);
    }

    let uptr_iov = UPtr::<IOVec>::from_uaddr(iov_base);
    let mut total = 0usize;
    for i in 0..iovcnt {
        let iov = uptr_iov.add(i).read()?;
        total = total.checked_add(iov.len).ok_or(Errno::EINVAL)?;
    }
    Ok(total)
}

fn gather_iov_data(iov_base: usize, iovcnt: usize) -> Result<Vec<u8>, Errno> {
    if iovcnt == 0 {
        return Ok(Vec::new());
    }

    let total = total_iov_len(iov_base, iovcnt)?;
    let uptr_iov = UPtr::<IOVec>::from_uaddr(iov_base);
    let mut out = vec![0u8; total];
    let mut offset = 0usize;
    for i in 0..iovcnt {
        let iov = uptr_iov.add(i).read()?;
        if iov.len == 0 {
            continue;
        }
        copy_from_user::buffer(iov.base, &mut out[offset..offset + iov.len]).map_err(|_| Errno::EFAULT)?;
        offset += iov.len;
    }
    Ok(out)
}

fn read_msg_name_sockaddr(msg: &UMsgHdr) -> SysResult<Option<SocketAddr>> {
    if msg.msg_name == 0 {
        return Ok(None);
    }

    if msg.msg_namelen as usize != size_of::<SockAddrIn>() {
        return Err(Errno::EINVAL);
    }

    read_inet_sockaddr(UPtr::<SockAddrIn>::from_uaddr(msg.msg_name), msg.msg_namelen as usize).map(Some)
}

fn send_msg(file: &Arc<dyn FileOps>, msg: &UMsgHdr, flags: usize) -> SyscallRet {
    msg.validate()?;

    let kbuf = gather_iov_data(msg.msg_iov, msg.msg_iovlen)?;
    let dst = read_msg_name_sockaddr(msg)?;
    send_socket(file, &kbuf, dst, flags)
}

pub fn sendmsg(fd: usize, uptr_msg: UPtr<UMsgHdr>, flags: usize) -> SyscallRet {
    let msg = uptr_msg.read()?;
    let file = current::fdtable().lock().get(fd)?;
    send_msg(&file, &msg, flags)
}

pub fn sendmmsg(fd: usize, uptr_msgvec: UPtr<UMMsgHdr>, vlen: usize, flags: usize) -> SyscallRet {
    if vlen == 0 {
        return Ok(0);
    }

    let file = current::fdtable().lock().get(fd)?;
    let mut sent_count = 0usize;
    for i in 0..vlen.min(MSG_IOV_MAX) {
        let uptr_mmsg = uptr_msgvec.add(i);
        let mmsg = match uptr_mmsg.read() {
            Ok(mmsg) => mmsg,
            Err(err) => {
                if sent_count > 0 {
                    return Ok(sent_count);
                }
                return Err(err);
            }
        };

        let sent = match send_msg(&file, &mmsg.msg_hdr, flags) {
            Ok(sent) => sent,
            Err(err) => {
                if sent_count > 0 {
                    return Ok(sent_count);
                }
                return Err(err);
            }
        };

        let uptr_msg_len = UPtr::<u32>::from_uaddr(uptr_mmsg.uaddr() + size_of::<UMsgHdr>());
        if let Err(err) = uptr_msg_len.write(sent.min(u32::MAX as usize) as u32) {
            if sent_count > 0 {
                return Ok(sent_count);
            }
            return Err(err);
        }

        sent_count += 1;
    }

    Ok(sent_count)
}

fn scatter_iov_data(iov_base: usize, iovcnt: usize, data: &[u8]) -> Result<usize, Errno> {
    if iovcnt == 0 || data.is_empty() {
        return Ok(0);
    }

    let uptr_iov = UPtr::<IOVec>::from_uaddr(iov_base);
    let mut copied = 0usize;
    for i in 0..iovcnt {
        if copied >= data.len() {
            break;
        }
        let iov = uptr_iov.add(i).read()?;
        if iov.len == 0 {
            continue;
        }
        let to_copy = (data.len() - copied).min(iov.len);
        copy_to_user::buffer(iov.base, &data[copied..copied + to_copy]).map_err(|_| Errno::EFAULT)?;
        copied += to_copy;
    }
    Ok(copied)
}

pub fn recvmsg(fd: usize, uptr_msg: UPtr<UMsgHdr>, flags: usize) -> SyscallRet {
    let mut msg = uptr_msg.read()?;
    msg.validate()?;
    msg.msg_flags = 0;
    msg.msg_namelen = 0;

    let total_len = total_iov_len(msg.msg_iov, msg.msg_iovlen)?;
    let file = current::fdtable().lock().get(fd)?;
    let mut kbuf = vec![0u8; total_len];
    let (n, src) = recv_socket(&file, &mut kbuf, flags)?;

    scatter_iov_data(msg.msg_iov, msg.msg_iovlen, &kbuf[..n])?;

    if msg.msg_name != 0 {
        if let Some(src) = src {
            let raw = src.to_raw();
            UPtr::<SockAddrIn>::from_uaddr(msg.msg_name).write(raw)?;
            msg.msg_namelen = core::mem::size_of::<SockAddrIn>() as u32;
        }
    }

    uptr_msg.write(msg)?;
    Ok(n)
}
