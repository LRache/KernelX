use alloc::sync::Arc;

use crate::fs::file::FileOps;
use crate::kernel::errno::Errno;
use crate::kernel::scheduler::current;
use crate::kernel::task::fdtable::FDFlags;
use crate::net::socket::{
    AF_INET, AF_NETLINK, InetSocket, NetlinkSocket, SOCK_CLOEXEC, SOCK_DGRAM, SOCK_NONBLOCK, SOCK_RAW, SOCK_STREAM,
    SockAddrIn, SocketAddr,
};

use super::SyscallRet;
use super::uptr::{UBuffer, UPtr, UserPointer};

pub fn socket(domain: usize, sock_type: usize, _protocol: usize) -> SyscallRet {
    let base_type = sock_type & !(SOCK_NONBLOCK | SOCK_CLOEXEC);
    let blocked = sock_type & SOCK_NONBLOCK == 0;
    let cloexec = sock_type & SOCK_CLOEXEC != 0;

    let sock: Arc<dyn FileOps> = match domain {
        AF_INET => match base_type {
            SOCK_DGRAM => Arc::new(InetSocket::new_udp(blocked)),
            SOCK_STREAM => Arc::new(InetSocket::new_tcp(blocked)),
            _ => return Err(Errno::EINVAL),
        },
        AF_NETLINK => match base_type {
            SOCK_RAW | SOCK_DGRAM => Arc::new(NetlinkSocket::new(blocked)),
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
