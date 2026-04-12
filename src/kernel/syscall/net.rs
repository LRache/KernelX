use alloc::sync::Arc;
use alloc::vec;
use alloc::vec::Vec;

use crate::fs::file::FileOps;
use crate::kernel::errno::Errno;
use crate::kernel::scheduler::current;
use crate::kernel::scheduler::current::{copy_from_user, copy_to_user};
use crate::kernel::task::fdtable::FDFlags;
use crate::net::socket::{
    AddressFamily, InetSocket, NetlinkProtocol, NetlinkSocket, SOCK_CLOEXEC, SOCK_NONBLOCK, SockAddrIn, SocketAddr,
    SocketKind,
};

use super::fs::IOVec;
use super::uptr::{UBuffer, UPtr, UserPointer};
use super::{SyscallRet, UserStruct};

const MSG_IOV_MAX: usize = 1024;

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

fn check_msg_hdr(msg: &UMsgHdr) -> Result<(), Errno> {
    if msg.msg_iovlen > MSG_IOV_MAX {
        return Err(Errno::EINVAL);
    }
    if msg.msg_iovlen > 0 && msg.msg_iov == 0 {
        return Err(Errno::EFAULT);
    }
    if msg.msg_control != 0 || msg.msg_controllen != 0 {
        return Err(Errno::EOPNOTSUPP);
    }
    Ok(())
}

fn gather_iov_data(iov_base: usize, iovcnt: usize) -> Result<Vec<u8>, Errno> {
    if iovcnt == 0 {
        return Ok(Vec::new());
    }

    let uptr_iov = UPtr::<IOVec>::from_uaddr(iov_base);
    let mut total = 0usize;
    for i in 0..iovcnt {
        let iov = uptr_iov.add(i).read()?;
        total = total.checked_add(iov.len).ok_or(Errno::EINVAL)?;
    }

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

pub fn socket(domain: usize, sock_type: usize, protocol: usize) -> SyscallRet {
    let base_type = sock_type & !(SOCK_NONBLOCK | SOCK_CLOEXEC);
    let blocked = sock_type & SOCK_NONBLOCK == 0;
    let cloexec = sock_type & SOCK_CLOEXEC != 0;
    let domain = AddressFamily::try_from(domain).map_err(|_| Errno::EAFNOSUPPORT)?;
    let sock_kind = SocketKind::try_from(base_type).map_err(|_| Errno::EINVAL)?;

    let sock: Arc<dyn FileOps> = match domain {
        AddressFamily::Inet => match sock_kind {
            SocketKind::Dgram => Arc::new(InetSocket::new_udp(blocked)),
            SocketKind::Stream => Arc::new(InetSocket::new_tcp(blocked)),
            SocketKind::Raw => {
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
        _ => return Err(Errno::EAFNOSUPPORT),
    };

    let fd = current::fdtable().lock().push(sock, FDFlags { cloexec })?;
    Ok(fd)
}

pub fn bind(fd: usize, addr_ptr: UPtr<SockAddrIn>, _addrlen: usize) -> SyscallRet {
    let file = current::fdtable().lock().get(fd)?;
    // Netlink sockets: bind is a no-op (kernel assigns pid automatically)
    if file.downcast_ref::<NetlinkSocket>().is_some() {
        return Ok(0);
    }
    let sock = file.downcast_ref::<InetSocket>().ok_or(Errno::ENOTSOCK)?;
    let raw = addr_ptr.read()?;
    let addr = SocketAddr::from_raw(&raw)?;
    sock.bind(addr)?;
    Ok(0)
}

pub fn listen(fd: usize, backlog: usize) -> SyscallRet {
    let file = current::fdtable().lock().get(fd)?;
    let sock = file.downcast_ref::<InetSocket>().ok_or(Errno::ENOTSOCK)?;
    sock.listen(backlog)?;
    Ok(0)
}

pub fn accept(fd: usize, addr_ptr: UPtr<SockAddrIn>, addrlen_ptr: UPtr<u32>) -> SyscallRet {
    let file = current::fdtable().lock().get(fd)?;
    let sock = file.downcast_ref::<InetSocket>().ok_or(Errno::ENOTSOCK)?;
    let new_sock = sock.accept()?;

    // Write peer address to user if requested
    if !addr_ptr.is_null() {
        // TODO: fill in peer address
    }

    let new_fd = current::fdtable()
        .lock()
        .push(new_sock as Arc<dyn FileOps>, FDFlags { cloexec: false })?;
    Ok(new_fd)
}

pub fn connect(fd: usize, addr_ptr: UPtr<SockAddrIn>, _addrlen: usize) -> SyscallRet {
    let file = current::fdtable().lock().get(fd)?;
    let sock = file.downcast_ref::<InetSocket>().ok_or(Errno::ENOTSOCK)?;
    let raw = addr_ptr.read()?;
    let addr = SocketAddr::from_raw(&raw)?;
    sock.connect(addr)?;
    Ok(0)
}

pub fn getsockname(fd: usize, addr_ptr: UPtr<SockAddrIn>, addrlen_ptr: UPtr<u32>) -> SyscallRet {
    if addr_ptr.is_null() || addrlen_ptr.is_null() {
        return Err(Errno::EFAULT);
    }

    let file = current::fdtable().lock().get(fd)?;
    if file.downcast_ref::<NetlinkSocket>().is_some() {
        return Err(Errno::EOPNOTSUPP);
    }

    let sock = file.downcast_ref::<InetSocket>().ok_or(Errno::ENOTSOCK)?;
    let addr = sock.local_addr().unwrap_or(SocketAddr::any(0)).to_raw();

    let user_len = addrlen_ptr.read()? as usize;
    let sockaddr_len = core::mem::size_of::<SockAddrIn>();
    if user_len < sockaddr_len {
        return Err(Errno::EINVAL);
    }

    addr_ptr.write(addr)?;
    addrlen_ptr.write(sockaddr_len as u32)?;
    Ok(0)
}

pub fn sendto(
    fd: usize,
    buf_ptr: UBuffer,
    len: usize,
    _flags: usize,
    addr_ptr: UPtr<SockAddrIn>,
    _addrlen: usize,
) -> SyscallRet {
    let file = current::fdtable().lock().get(fd)?;

    let mut kbuf = alloc::vec![0u8; len];
    buf_ptr.read(0, &mut kbuf)?;

    // Netlink: sendto goes through write (address is ignored)
    if file.downcast_ref::<NetlinkSocket>().is_some() {
        return file.write(&kbuf);
    }

    let sock = file.downcast_ref::<InetSocket>().ok_or(Errno::ENOTSOCK)?;
    let dst = if !addr_ptr.is_null() {
        let raw = addr_ptr.read()?;
        Some(SocketAddr::from_raw(&raw)?)
    } else {
        None
    };

    sock.sendto(&kbuf, dst)
}

pub fn recvfrom(
    fd: usize,
    buf_ptr: UBuffer,
    len: usize,
    _flags: usize,
    addr_ptr: UPtr<SockAddrIn>,
    addrlen_ptr: UPtr<u32>,
) -> SyscallRet {
    let file = current::fdtable().lock().get(fd)?;

    // Netlink: recvfrom goes through read (source address zeroed)
    if file.downcast_ref::<NetlinkSocket>().is_some() {
        let mut kbuf = alloc::vec![0u8; len];
        let n = file.read(&mut kbuf)?;
        buf_ptr.write(0, &kbuf[..n])?;
        return Ok(n);
    }

    let sock = file.downcast_ref::<InetSocket>().ok_or(Errno::ENOTSOCK)?;

    let mut kbuf = alloc::vec![0u8; len];
    let (n, src) = sock.recvfrom(&mut kbuf)?;

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

pub fn setsockopt(fd: usize, level: usize, optname: usize, optval: usize, optlen: usize) -> SyscallRet {
    let file = current::fdtable().lock().get(fd)?;
    if file.downcast_ref::<NetlinkSocket>().is_some() {
        return Ok(0);
    }
    let sock = file.downcast_ref::<InetSocket>().ok_or(Errno::ENOTSOCK)?;
    sock.setsockopt(level, optname, optval, optlen)?;
    Ok(0)
}

pub fn getsockopt(fd: usize, level: usize, optname: usize, optval: usize, optlen: usize) -> SyscallRet {
    let file = current::fdtable().lock().get(fd)?;
    if file.downcast_ref::<NetlinkSocket>().is_some() {
        return Ok(0);
    }
    let sock = file.downcast_ref::<InetSocket>().ok_or(Errno::ENOTSOCK)?;
    sock.getsockopt(level, optname, optval, optlen)?;
    Ok(0)
}

pub fn shutdown(fd: usize, how: usize) -> SyscallRet {
    let file = current::fdtable().lock().get(fd)?;
    if file.downcast_ref::<NetlinkSocket>().is_some() {
        return Ok(0);
    }
    let sock = file.downcast_ref::<InetSocket>().ok_or(Errno::ENOTSOCK)?;
    sock.shutdown(how)?;
    Ok(0)
}

pub fn sendmsg(fd: usize, uptr_msg: UPtr<UMsgHdr>, _flags: usize) -> SyscallRet {
    let msg = uptr_msg.read()?;
    check_msg_hdr(&msg)?;

    let file = current::fdtable().lock().get(fd)?;
    let kbuf = gather_iov_data(msg.msg_iov, msg.msg_iovlen)?;

    // Netlink sendmsg follows sendto semantics in this kernel: destination address ignored.
    if file.downcast_ref::<NetlinkSocket>().is_some() {
        return file.write(&kbuf);
    }

    let sock = file.downcast_ref::<InetSocket>().ok_or(Errno::ENOTSOCK)?;
    let dst = if msg.msg_name != 0 {
        if msg.msg_namelen as usize != core::mem::size_of::<SockAddrIn>() {
            return Err(Errno::EINVAL);
        }
        let raw = UPtr::<SockAddrIn>::from_uaddr(msg.msg_name).read()?;
        Some(SocketAddr::from_raw(&raw)?)
    } else {
        None
    };

    sock.sendto(&kbuf, dst)
}

pub fn recvmsg(fd: usize, uptr_msg: UPtr<UMsgHdr>, _flags: usize) -> SyscallRet {
    let mut msg = uptr_msg.read()?;
    check_msg_hdr(&msg)?;
    msg.msg_flags = 0;
    msg.msg_namelen = 0;

    let mut total_len = 0usize;
    let uptr_iov = UPtr::<IOVec>::from_uaddr(msg.msg_iov);
    for i in 0..msg.msg_iovlen {
        let iov = uptr_iov.add(i).read()?;
        total_len = total_len.checked_add(iov.len).ok_or(Errno::EINVAL)?;
    }

    let file = current::fdtable().lock().get(fd)?;
    let mut kbuf = vec![0u8; total_len];

    let (n, src) = if file.downcast_ref::<NetlinkSocket>().is_some() {
        (file.read(&mut kbuf)?, None)
    } else {
        let sock = file.downcast_ref::<InetSocket>().ok_or(Errno::ENOTSOCK)?;
        sock.recvfrom(&mut kbuf)?
    };

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
