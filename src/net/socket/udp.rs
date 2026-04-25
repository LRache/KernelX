use alloc::sync::Arc;

use crate::kernel::errno::{Errno, SysResult};
use crate::kernel::event::Event;
use crate::kernel::scheduler::current;
use crate::net::interface::Interface;
use crate::net::manager;

use super::{SocketAddr, SocketInner};

pub struct UdpInner {
    pub local: Option<SocketAddr>,
    pub remote: Option<SocketAddr>,
    iface: Option<Arc<Interface>>,
}

impl UdpInner {
    pub fn new() -> Self {
        Self {
            local: None,
            remote: None,
            iface: None,
        }
    }

    fn resolve_iface(&mut self) -> SysResult<Arc<Interface>> {
        if let Some(ref iface) = self.iface {
            return Ok(iface.clone());
        }
        let iface = manager::default_interface().ok_or(Errno::ENETUNREACH)?;
        self.iface = Some(iface.clone());
        Ok(iface)
    }
}

impl SocketInner for UdpInner {
    fn bind(&mut self, addr: SocketAddr) -> SysResult<()> {
        if self.local.is_some() {
            return Err(Errno::EINVAL);
        }
        if addr.port != 0 && addr.port < 1024 && current::euid() != 0 {
            return Err(Errno::EACCES);
        }

        let iface = if addr.ip.is_unspecified() {
            manager::default_interface().ok_or(Errno::EADDRNOTAVAIL)?
        } else {
            manager::find_interface_for(addr.ip).ok_or(Errno::EADDRNOTAVAIL)?
        };

        iface.bind_udp(addr.port);
        self.local = Some(addr);
        self.iface = Some(iface);
        Ok(())
    }

    fn connect(&mut self, addr: SocketAddr, _blocked: bool) -> SysResult<()> {
        self.remote = Some(addr);
        // Auto-bind if not already bound
        if self.local.is_none() {
            let iface = self.resolve_iface()?;
            let port = iface.alloc_ephemeral_udp_port();
            iface.bind_udp(port);
            self.local = Some(SocketAddr::any(port));
        }
        Ok(())
    }

    fn sendto(&mut self, buf: &[u8], dst: Option<SocketAddr>, _blocked: bool) -> SysResult<usize> {
        let dst = dst.or(self.remote).ok_or(Errno::EDESTADDRREQ)?;

        // Auto-bind if not bound
        if self.local.is_none() {
            let iface = self.resolve_iface()?;
            let port = iface.alloc_ephemeral_udp_port();
            iface.bind_udp(port);
            self.local = Some(SocketAddr::any(port));
        }

        let iface = self.iface.as_ref().unwrap();
        let local = self.local.unwrap();

        iface.send_udp(local, dst, buf)?;
        Ok(buf.len())
    }

    fn recvfrom(&mut self, buf: &mut [u8], blocked: bool) -> SysResult<(usize, Option<SocketAddr>)> {
        let local = self.local.ok_or(Errno::EINVAL)?;
        let iface = self.iface.as_ref().ok_or(Errno::EINVAL)?;

        loop {
            match iface.try_recv_udp(local.port) {
                Some((src, data)) => {
                    let n = buf.len().min(data.len());
                    buf[..n].copy_from_slice(&data[..n]);
                    return Ok((n, Some(src)));
                }
                None => {
                    if !blocked {
                        return Err(Errno::EAGAIN);
                    }
                    iface.wait_udp(local.port);
                    current::schedule();

                    match current::task().take_wakeup_event() {
                        Some(Event::ReadReady) => continue,
                        Some(Event::Signal) => return Err(Errno::EINTR),
                        _ => continue,
                    }
                }
            }
        }
    }

    fn poll_read(&self) -> bool {
        if let (Some(local), Some(iface)) = (&self.local, &self.iface) {
            iface.has_udp_data(local.port)
        } else {
            false
        }
    }

    fn type_name(&self) -> &'static str {
        "inet-udp"
    }

    fn local_addr(&self) -> Option<SocketAddr> {
        self.local
    }
}

impl Drop for UdpInner {
    fn drop(&mut self) {
        if let (Some(local), Some(iface)) = (self.local.take(), self.iface.take()) {
            iface.unbind_udp(local.port);
        }
    }
}
