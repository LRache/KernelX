use core::net::Ipv4Addr;

use alloc::sync::Arc;

use crate::kernel::errno::{Errno, SysResult};
use crate::kernel::event::Event;
use crate::kernel::scheduler::current;
use crate::net::interface::Interface;
use crate::net::manager;

use super::{SocketAddr, SocketInner};

pub struct RawInner {
    protocol: u8,
    local: Option<SocketAddr>,
    remote: Option<SocketAddr>,
    iface: Option<Arc<Interface>>,
}

impl RawInner {
    pub fn new(protocol: u8) -> Self {
        Self {
            protocol,
            local: None,
            remote: None,
            iface: None,
        }
    }

    fn resolve_iface(&mut self, ip: core::net::Ipv4Addr) -> SysResult<Arc<Interface>> {
        if let Some(ref iface) = self.iface {
            return Ok(iface.clone());
        }
        let iface = if ip.is_unspecified() {
            manager::default_interface()
        } else {
            manager::find_interface_for(ip)
        }
        .ok_or(Errno::ENETUNREACH)?;
        self.iface = Some(iface.clone());
        Ok(iface)
    }

    fn ensure_bound(&mut self) -> SysResult<()> {
        if self.local.is_some() && self.iface.is_some() {
            return Ok(());
        }
        let iface = self.resolve_iface(core::net::Ipv4Addr::UNSPECIFIED)?;
        iface.bind_raw(self.protocol);
        self.local = Some(SocketAddr::new(iface.ipv4().unwrap_or(Ipv4Addr::UNSPECIFIED), 0));
        Ok(())
    }
}

impl SocketInner for RawInner {
    fn bind(&mut self, addr: SocketAddr) -> SysResult<()> {
        if self.local.is_some() {
            return Err(Errno::EINVAL);
        }
        if addr.port != 0 {
            return Err(Errno::EINVAL);
        }

        let iface = self.resolve_iface(addr.ip)?;
        iface.bind_raw(self.protocol);
        self.local = Some(SocketAddr::new(addr.ip, 0));
        Ok(())
    }

    fn connect(&mut self, addr: SocketAddr, _blocked: bool) -> SysResult<()> {
        self.remote = Some(SocketAddr::new(addr.ip, 0));
        self.ensure_bound()
    }

    fn sendto(&mut self, buf: &[u8], dst: Option<SocketAddr>, _blocked: bool) -> SysResult<usize> {
        let dst = dst.or(self.remote).ok_or(Errno::EDESTADDRREQ)?;
        self.ensure_bound()?;

        let iface = self.iface.as_ref().ok_or(Errno::ENETUNREACH)?;
        let src_ip = self.local.map(|a| a.ip).unwrap_or(core::net::Ipv4Addr::UNSPECIFIED);
        iface.send_raw(src_ip, dst.ip, self.protocol, buf)?;
        Ok(buf.len())
    }

    fn recvfrom(&mut self, buf: &mut [u8], blocked: bool) -> SysResult<(usize, Option<SocketAddr>)> {
        self.ensure_bound()?;
        let iface = self.iface.as_ref().ok_or(Errno::ENETUNREACH)?;

        loop {
            match iface.try_recv_raw(self.protocol) {
                Some((src, data)) => {
                    let n = buf.len().min(data.len());
                    buf[..n].copy_from_slice(&data[..n]);
                    return Ok((n, Some(src)));
                }
                None => {
                    if !blocked {
                        return Err(Errno::EAGAIN);
                    }
                    iface.wait_raw(self.protocol);
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
        if let Some(iface) = &self.iface {
            iface.has_raw_data(self.protocol)
        } else {
            false
        }
    }

    fn type_name(&self) -> &'static str {
        "inet-raw"
    }

    fn local_addr(&self) -> Option<SocketAddr> {
        self.local
    }
}

impl Drop for RawInner {
    fn drop(&mut self) {
        if let Some(iface) = self.iface.take() {
            iface.unbind_raw(self.protocol);
        }
    }
}
